//! The embedded Tuta web app.
//!
//! Tuta has no IMAP endpoint and no local bridge: its account exists only in
//! the web app at <https://app.tuta.com>. Nextcloud shows Tuta the same way,
//! by embedding that web app, so Yours does too.
//!
//! # Why a separate process
//!
//! Slint's winit backend owns the application's event loop on the main
//! thread. The GTK/WebKitGTK stack that paints the web view requires that
//! same main thread: `gtk::init` and tao's event loop both assert it, and two
//! event loops cannot share one thread. The web app therefore runs as
//! `yours --tuta-webview <data dir>`, a child of the app that owns the main
//! thread of its own process.
//!
//! # It stays in the background
//!
//! The child is started at Yours' startup with its window never mapped, but
//! realized: the page loads, keeps its Tuta session and keeps reading new
//! mail, so a notification it raises is forwarded even while no window is on
//! screen. Closing the window hides it instead of ending the process; the
//! sidebar reopens the tab, whose button shows the window again. The child
//! watches its parent and terminates when Yours exits, so quitting Yours ends
//! the web app rather than leaving an orphan behind.
//!
//! # The tab
//!
//! The owner asked for Tuta inside the Yours window. On Wayland a client
//! cannot reparent another client's surface into its own: there is no
//! cross-client subsurface or xdg-foreign reparent, and Yours itself runs on
//! Wayland (`winit`'s Wayland backend), so XReparentWindow has no X11 parent
//! window to attach to. WebKitGTK has no offscreen render API either
//! (`webkit_web_view_get_snapshot` is the only way to get the page as an
//! image), and Slint's winit backend exposes no native parent a GTK widget
//! could be put into — both require the process's main thread and a surface
//! they alone own. The tab is therefore the best shippable thing: the web
//! view's frames through WebKitGTK's own snapshot API, in a tab-shaped page
//! inside Yours, with a clear note that it is a picture and the window opens
//! for clicks and typing.
//!
//! # Profile
//!
//! WebKitGTK keeps cookies, localStorage and IndexedDB under
//! `<data dir>/tuta/webview`, so the Tuta session survives restarts and never
//! mixes with the mail profile.
//!
//! # Notifications
//!
//! The page's `Notification` permission request is granted explicitly
//! (everything else is denied); without that WebKitGTK silently denies it.
//! The page's notifications are then handed to the web view's
//! `show-notification` signal rather than reaching the session bus: the child
//! forwards the line to the app over a private socket, where
//! `notify_bridge::announce_web_notification` applies the same switch, quiet
//! hours, sender extraction and dedupe as the bus listener and speaks it. So
//! the bus bridge keeps covering the desktop Tuta application while this path
//! covers Yours' own web view. When the bridge declines (off, quiet hours, no
//! speech engine) the handler declines too, and WebKitGTK shows the
//! notification normally.

use std::{
    error::Error,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::FileTypeExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use flectar_mail_core::config::Paths;
use gtk::prelude::*;
use slint::ComponentHandle;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::unix::WindowExtUnix,
    window::{Icon, WindowBuilder},
};
// The PNG writer comes from the same cairo-rs the GTK/WebKitGTK bindings use;
// naming the crate marks it as a real dependency (its "png" feature is what
// turns a snapshot surface into a frame).
use cairo as _;
use webkit2gtk::{NotificationExt, SnapshotOptions, SnapshotRegion, WebViewExt};
use wry::{
    PermissionKind, PermissionResponse, WebContext, WebViewBuilder, WebViewBuilderExtUnix,
    WebViewExtUnix,
};

/// The argument that turns this executable into the Tuta web app window.
const ARG: &str = "--tuta-webview";
/// Keeps the window unmapped on start: the web app runs in the background.
const HIDDEN_ARG: &str = "--hidden";
/// The official web app; there is no other Tuta endpoint.
const URL: &str = "https://app.tuta.com/";
/// Loads a different page in the same window, for testing the notification
/// path without a Tuta login. The app itself never sets it.
const URL_ENV: &str = "YOURS_TUTA_WEB_APP_URL";
/// Honest title: this window is the web app.
const TITLE: &str = "Tuta web app";
/// The app's desktop-entry id, so docks group this window with Yours and show
/// the quill installed with it.
const APP_ID: &str = "org.yours.mail";
/// What the application calls itself on the session bus; WebKitGTK uses this
/// as the app name of the notifications it raises for the page.
const APP_NAME: &str = "Yours";
/// The quill, for window managers that take the icon from the window rather
/// than from the desktop entry.
const ICON: &[u8] = include_bytes!("../resources/app-icon/icon-seal-256.png");
const WIDTH: f64 = 1180.0;
const HEIGHT: f64 = 900.0;
/// Longest notification line handed from the window to the app.
const MAX_NOTIFICATION_BYTES: u64 = 8 * 1024;
/// How often the tab asks the web view for a frame while it is open. A frame
/// is a PNG through WebKitGTK's snapshot API; once a second keeps the tab
/// current without painting a video nobody asked for.
const FRAME_INTERVAL: Duration = Duration::from_millis(900);
/// A frame larger than this is not a frame; refuse it rather than allocate.
const MAX_FRAME_BYTES: u32 = 24 * 1024 * 1024;
/// Marks that the owner has had the interactive window on screen at least
/// once. The web app only starts hidden from then on: a first run has to show
/// the window, because a snapshot tab cannot be logged into.
const SHOWN_MARKER: &str = "webview-shown";
/// How long a starting window may take to answer its socket.
const FOCUS_ATTEMPTS: u32 = 60;
const FOCUS_RETRY: Duration = Duration::from_millis(50);
const SOCKET_TIMEOUT: Duration = Duration::from_millis(500);
/// After asking for a child, give it this long to take its lock before a
/// second request may spawn another one. The child owns the lock for its
/// whole lifetime, so this only covers the start-up race.
const SPAWN_GRACE: Duration = Duration::from_secs(5);

/// Events the sockets deliver to the window's event loop.
enum Wake {
    /// Bring the window to the front.
    Show,
    /// Hide the window, keeping the page and the process alive.
    Hide,
    /// Start or stop sending frames for the open tab.
    Frames(bool),
    /// Exit the child (used by manual checks; Yours' exit signal is the
    /// parent-death signal).
    Quit,
}

/// The child process: returns `None` in the normal app, `Some(exit code)`
/// when this executable was started as the Tuta web app window.
pub fn run_if_requested() -> Option<i32> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(ARG)) {
        return None;
    }
    Some(match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("tuta: {error}");
            1
        }
    })
}

/// Wire the sidebar entry and the tab.
///
/// The child is started here, at Yours' startup, and kept in the background
/// whether or not the tab is ever opened. The frame listener and the
/// notification listener live in the app; the child reaches them over the
/// private per-user sockets.
pub fn install(app: &crate::AppWindow, data_dir: &Path) {
    // The window's notifications are handed back here, where the bridge, the
    // settings store and the speech engine live. A listener that cannot start
    // costs only that speech, never the window.
    let notifications = notify_socket_path(data_dir);
    let _ = std::thread::Builder::new()
        .name("tuta-notify".into())
        .spawn(move || serve_notifications(&notifications));
    // The open tab is painted from these frames.
    let frames = frame_socket_path(data_dir);
    serve_frames(app.as_weak(), frames);

    let global = app.global::<crate::TutaWebApp>();
    global.set_available(true);

    // A test seam: YOURS_TUTA_TAB opens the tab at start-up so the frame path
    // can be checked without a click. The app itself never sets it.
    if std::env::var_os("YOURS_TUTA_TAB").is_some() {
        app.set_active_view("tuta".into());
    }

    // Once the interactive window has been on screen (the owner had the
    // chance to log in), start the web app hidden at every launch: its Tuta
    // session is kept warm and its notifications are read aloud even if the
    // window is never opened. A first run waits for the sidebar, which shows
    // the window because a snapshot tab cannot be logged into. The spawn runs
    // on the main thread because the child watches that thread for its
    // parent's exit.
    if profile_used(data_dir) {
        let startup_dir = data_dir.to_owned();
        if let Err(error) = ensure_running(&startup_dir) {
            eprintln!("tuta: could not start the web app in the background: {error}");
        }
    }

    // The sidebar's row opens the tab once the web app has been used; on a
    // first run it shows the interactive window so the owner can log in.
    let open_dir = data_dir.to_owned();
    let open_app = app.as_weak();
    global.on_open(move || {
        if profile_used(&open_dir) {
            if let Some(app) = open_app.upgrade() {
                app.set_active_view("tuta".into());
            }
            if let Err(error) = ensure_running(&open_dir) {
                eprintln!("tuta: could not start the web app: {error}");
            }
        } else if let Err(error) = show_window(&open_dir) {
            eprintln!("tuta: could not show the web app window: {error}");
        }
    });

    // "Open window" in the tab shows the interactive window, or brings it
    // back after it was closed to the background.
    let show_dir = data_dir.to_owned();
    global.on_show_window(move || {
        if let Err(error) = show_window(&show_dir) {
            eprintln!("tuta: could not show the web app window: {error}");
        }
    });

    // The tab was opened or left: while it is open, ask the child for frames.
    let frames_dir = data_dir.to_owned();
    global.on_live_view(move |live| {
        let socket = socket_path(&frames_dir);
        let message = if live { "frames on" } else { "frames off" };
        let _ = std::thread::Builder::new()
            .name("tuta-frames-request".into())
            .spawn(move || {
                send_until(&socket, message, FOCUS_ATTEMPTS);
            });
    });
}

/// Start the web app process if it is not running. The child owns
/// `tuta/webview.lock` for its whole lifetime, so a second process cannot
/// start; a lock we can take proves none is running.
fn ensure_running(data_dir: &Path) -> io::Result<()> {
    let socket = socket_path(data_dir);
    if is_listening(&socket) {
        return Ok(());
    }
    let last_spawn = last_spawn();
    let mut last = last_spawn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if last.is_some_and(|at| at.elapsed() < SPAWN_GRACE) {
        // A child we just asked for is still starting; do not start a second
        // one that would block on the lock forever.
        return Ok(());
    }
    match InstanceLock::try_acquire(data_dir)? {
        None => Ok(()),
        Some(lock) => {
            // The lock proves no window is serving this profile; a socket
            // file without a listener is debris from a window that was killed.
            remove_our_socket(&socket);
            let executable = std::env::current_exe()?;
            Command::new(executable)
                .arg(ARG)
                .arg(data_dir)
                .arg(HIDDEN_ARG)
                .arg(std::process::id().to_string())
                // The window has no console; report nothing through ours.
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            *last = Some(Instant::now());
            drop(lock);
            Ok(())
        }
    }
}

/// Show the interactive window, whether it is hidden or was never mapped.
fn show_window(data_dir: &Path) -> io::Result<()> {
    // Starting the child touches the parent-death binding, so it stays on the
    // main thread; asking a starting child to show is retried off it.
    ensure_running(data_dir)?;
    let socket = socket_path(data_dir);
    let _ = std::thread::Builder::new()
        .name("tuta-show".into())
        .spawn(move || {
            send_until(&socket, "show", FOCUS_ATTEMPTS);
        });
    Ok(())
}

/// The child process: one web view, one event loop, and a window that hides
/// instead of closing.
fn run() -> Result<(), Box<dyn Error>> {
    // The parent passes its profile so the web app follows the same data
    // directory as the mail it belongs to; a manual launch falls back to the
    // default profile.
    let data_dir = match std::env::args_os().nth(2) {
        Some(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => Paths::default_dirs()?.data_dir,
    };
    // Yours starts the web app hidden; the sidebar's window button shows it.
    let hidden = std::env::args_os().nth(3).as_deref() == Some(OsStr::new(HIDDEN_ARG));
    // The child ends with Yours: quitting the app must not leave an orphan
    // web app behind. Manual launches pass no parent.
    let parent = std::env::args_os()
        .nth(4)
        .and_then(|value| value.to_str().and_then(|text| text.parse::<i32>().ok()));
    arm_parent_death(parent);

    let profile = data_dir.join("tuta").join("webview");
    fs::create_dir_all(&profile)?;

    // Held until this process exits: it is how the sidebar knows a window is
    // running. The lock file stays; only the lock is released.
    let _instance = InstanceLock::acquire(&data_dir)?;
    let socket = socket_path(&data_dir);
    let listener = match FocusListener::bind(&socket) {
        Ok(listener) => listener,
        // Another window won the race; forward the request and leave.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = send(&socket, "show");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    // Names other programs see: the dock puts the window under Yours and
    // shows the quill, and WebKitGTK names Yours as the origin of the page's
    // notifications.
    gtk::glib::set_prgname(Some(APP_ID));
    gtk::glib::set_application_name(APP_NAME);

    let event_loop = EventLoopBuilder::<Wake>::with_user_event().build();
    listener.serve(event_loop.create_proxy());

    let mut builder = WindowBuilder::new()
        .with_title(TITLE)
        .with_visible(!hidden)
        // Realize the widget tree without mapping it: WebKitGTK can paint a
        // realized, unmapped view, so the tab has frames and the page keeps
        // running while no window is on screen.
        .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
        .with_min_inner_size(LogicalSize::new(480.0, 360.0));
    if let Some(icon) = icon() {
        builder = builder.with_window_icon(Some(icon));
    }
    let window = builder.build(&event_loop)?;
    window.gtk_window().set_icon_name(Some(APP_ID));
    if hidden {
        window.gtk_window().realize();
    } else {
        // A shown window is the owner's chance to log in; later launches can
        // keep the web app hidden in the background.
        mark_shown(&data_dir);
    }

    // The profile is WebKitGTK's website data store: cookies (in a persistent
    // cookie file), localStorage, IndexedDB and service workers all survive
    // restarts. The context must outlive the web view, so it stays here.
    let page = std::env::var(URL_ENV).unwrap_or_else(|_| URL.to_owned());
    let mut context = WebContext::new(Some(profile));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&page)
        .with_permission_handler(|kind| match kind {
            // Tuta asks once, and WebKitGTK remembers the answer in the
            // profile. Without this the browser default silently denies.
            PermissionKind::Notifications => PermissionResponse::Allow,
            // A mail client's web view has no business with a camera,
            // microphone, location, or reading the clipboard.
            _ => PermissionResponse::Deny,
        })
        .build_gtk(window.default_vbox().ok_or("tao made no GTK container")?)?;
    if hidden {
        // Best effort: a child widget is realized when its parent is, but the
        // snapshot needs the web view itself realized before the first map.
        webview.webview().realize();
    }

    // A page notification is handed to the app, not to the session bus. The
    // answer decides whether WebKitGTK still shows it: when the bridge spoke
    // the line it returns true and the notification stays off the bus and the
    // screen; when the bridge declines, false lets WebKitGTK show it normally.
    let notify_socket = notify_socket_path(&data_dir);
    webview.webview().connect_show_notification(move |_view, notification| {
        let summary = notification
            .title()
            .map(|title| title.to_string())
            .unwrap_or_default();
        let body = notification
            .body()
            .map(|body| body.to_string())
            .unwrap_or_default();
        let announced = notify_app(&notify_socket, &summary, &body);
        // The window usually has no console; a manual run still sees why a
        // line was or was not spoken.
        eprintln!("tuta: notification {summary:?} handed over (announced={announced})");
        announced
    });

    // The open tab's frames. The producer only runs while the tab asks: the
    // timer checks the flag, and the writer thread drops the connection while
    // frames are off, so a background window costs nothing.
    let frames_on = Arc::new(AtomicBool::new(false));
    let frame_socket = frame_socket_path(&data_dir);
    let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<u8>>(1);
    spawn_frame_writer(frame_socket, frames_on.clone(), frame_rx);
    schedule_frames(&webview, frames_on.clone(), frame_tx);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Wake::Show) => {
                window.set_visible(true);
                window.set_focus();
                mark_shown(&data_dir);
            }
            Event::UserEvent(Wake::Hide) => window.set_visible(false),
            Event::UserEvent(Wake::Frames(on)) => frames_on.store(on, Ordering::Relaxed),
            Event::UserEvent(Wake::Quit) => *control_flow = ControlFlow::Exit,
            // Closing the window hides it: the page, the Tuta session and the
            // notification path stay alive until Yours itself exits.
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                window.set_visible(false);
                eprintln!("tuta: window hidden; the web app keeps running in the background");
            }
            // A destroyed window cannot be shown again; end the process.
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
        // The web view lives exactly as long as this window and must stay
        // owned by the event loop's closure.
        let _ = &webview;
    })
}

/// The quill as a window icon, decoded from the PNG bundled for the app icon.
fn icon() -> Option<Icon> {
    let image = image::load_from_memory(ICON).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// End this process when the Yours main thread ends: quitting Yours ends the
/// web app. The expected parent id is checked so a child whose parent already
/// died does not linger as a page load.
fn arm_parent_death(expected: Option<i32>) {
    let Some(expected) = expected else {
        return;
    };
    // SAFETY: prctl only sets a process attribute; no pointer is involved.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
    // SAFETY: getppid takes no arguments and cannot fail.
    if unsafe { libc::getppid() } != expected {
        std::process::exit(0);
    }
}

/// Where the "interactive window has been shown at least once" marker lives.
fn shown_marker(data_dir: &Path) -> PathBuf {
    data_dir.join("tuta").join(SHOWN_MARKER)
}

/// Whether the owner has seen the interactive window before, and so no longer
/// needs a first run that opens it for logging in.
fn profile_used(data_dir: &Path) -> bool {
    shown_marker(data_dir).is_file()
}

/// Remember that the interactive window has been on screen, so later launches
/// can start the web app hidden.
fn mark_shown(data_dir: &Path) {
    let marker = shown_marker(data_dir);
    if let Some(directory) = marker.parent() {
        let _ = fs::create_dir_all(directory);
    }
    let _ = fs::write(marker, b"1\n");
}

/// The socket the sidebar uses to bring the window to the front, or the one
/// the window uses to hand a notification to the app; both live in the private
/// per-user runtime directory, so they disappear at logout.
fn socket_path(data_dir: &Path) -> PathBuf {
    runtime_socket(data_dir, "focus", "webview.sock")
}

fn notify_socket_path(data_dir: &Path) -> PathBuf {
    runtime_socket(data_dir, "notify", "notify.sock")
}

fn frame_socket_path(data_dir: &Path) -> PathBuf {
    runtime_socket(data_dir, "frames", "frames.sock")
}

fn runtime_socket(data_dir: &Path, kind: &str, fallback: &str) -> PathBuf {
    let key = crate::single_instance::profile_key(data_dir);
    match std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|directory| directory.is_absolute() && directory.is_dir())
    {
        Some(runtime) => runtime.join(format!("org.yours.mail-tuta-{kind}-{key}.sock")),
        None => data_dir.join("tuta").join(fallback),
    }
}

/// The app's end of the notification socket: every line the child forwards is
/// spoken here, and the answer tells the child whether to suppress WebKitGTK's
/// own display.
fn serve_notifications(socket: &Path) {
    if let Some(directory) = socket.parent() {
        let _ = fs::create_dir_all(directory);
    }
    remove_our_socket(socket);
    let Ok(listener) = UnixListener::bind(socket) else {
        eprintln!(
            "tuta: cannot listen for the web app's notifications at {}",
            socket.display()
        );
        return;
    };
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(SOCKET_TIMEOUT));
        // One message per connection: the child writes it, then waits for the
        // answer, so this must not read to end of file.
        let mut buffer = vec![0u8; MAX_NOTIFICATION_BYTES as usize];
        let Ok(count) = stream.read(&mut buffer) else {
            continue;
        };
        let payload = String::from_utf8_lossy(&buffer[..count]);
        let (summary, body) = payload.split_once('\t').unwrap_or((payload.trim_end(), ""));
        let announced = crate::notify_bridge::announce_web_notification(summary, body);
        let _ = stream.write_all(if announced { b"1" } else { b"0" });
    }
}

/// The child's half: hand one notification to the app and report whether it
/// was spoken. A false answer, or an app that cannot be reached, means
/// WebKitGTK shows the notification normally instead.
fn notify_app(socket: &Path, summary: &str, body: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(750)));
    let summary: String = summary.chars().take(200).collect();
    let body: String = body.chars().take(500).collect();
    if stream
        .write_all(format!("{summary}\t{body}").as_bytes())
        .is_err()
    {
        return false;
    }
    // The app reads one message; closing this direction lets it finish even
    // without a newline, and the reply still comes back the other way.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut answer = [0u8; 1];
    matches!(stream.read(&mut answer), Ok(1) if answer[0] == b'1')
}

/// The app's end of the frame socket: the child sends length-prefixed PNGs,
/// which become the tab's image. Decoding happens here, off the UI thread.
fn serve_frames(app: slint::Weak<crate::AppWindow>, socket: PathBuf) {
    static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
    let _ = std::thread::Builder::new()
        .name("tuta-frames".into())
        .spawn(move || {
            if let Some(directory) = socket.parent() {
                let _ = fs::create_dir_all(directory);
            }
            remove_our_socket(&socket);
            let Ok(listener) = UnixListener::bind(&socket) else {
                eprintln!(
                    "tuta: cannot listen for the web app's frames at {}",
                    socket.display()
                );
                return;
            };
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                loop {
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).is_err() {
                        break;
                    }
                    let length = u32::from_be_bytes(header);
                    if length == 0 || length > MAX_FRAME_BYTES {
                        break;
                    }
                    let mut png = vec![0u8; length as usize];
                    if stream.read_exact(&mut png).is_err() {
                        break;
                    }
                    // The pixels, not the decoded Slint image, travel to the
                    // UI thread: an Image is not Send, and building it there
                    // also keeps the copy on the thread that draws it.
                    let Some(frame) = frame_pixels(&png) else {
                        continue;
                    };
                    if !FIRST_FRAME.swap(true, Ordering::Relaxed) {
                        // The tab is a picture, not an embed: this line is how
                        // a live check proves the snapshot path works.
                        eprintln!("tuta: tab frame received ({} bytes of PNG)", png.len());
                    }
                    let app = app.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app.upgrade() {
                            let image = slint::Image::from_rgba8(
                                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                    &frame.pixels,
                                    frame.width,
                                    frame.height,
                                ),
                            );
                            let global = app.global::<crate::TutaWebApp>();
                            global.set_has_frame(true);
                            global.set_frame(image);
                        }
                    });
                }
            }
        });
}

/// One decoded snapshot: un-premultiplied RGBA pixels and their size.
struct Frame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decode one snapshot frame's PNG, or `None` when it is not a decodable
/// image.
fn frame_pixels(png: &[u8]) -> Option<Frame> {
    let image = image::load_from_memory(png).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(Frame {
        pixels: image.into_raw(),
        width,
        height,
    })
}

/// The child's frame producer: a UI-thread timer asks WebKitGTK for a snapshot
/// while frames are on, and the PNG goes to the writer thread.
fn schedule_frames(webview: &wry::WebView, frames_on: Arc<AtomicBool>, sender: mpsc::SyncSender<Vec<u8>>) {
    let view = webview.webview().clone();
    let pending = Arc::new(AtomicBool::new(false));
    gtk::glib::timeout_add_local(FRAME_INTERVAL, move || {
        if !frames_on.load(Ordering::Relaxed) || pending.swap(true, Ordering::SeqCst) {
            return gtk::glib::ControlFlow::Continue;
        }
        let pending = pending.clone();
        let sender = sender.clone();
        view.snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                pending.store(false, Ordering::SeqCst);
                let Ok(surface) = result else {
                    return;
                };
                let mut png = Vec::new();
                if surface.write_to_png(&mut png).is_ok() && !png.is_empty() {
                    // Keep the newest frame; a slow app drops old ones rather
                    // than queueing a video behind the tab.
                    let _ = sender.try_send(png);
                }
            },
        );
        gtk::glib::ControlFlow::Continue
    });
}

/// The child's frame writer: one connection to the app, one length-prefixed
/// PNG per frame. Connecting lazily and dropping the connection while frames
/// are off keeps a hidden window free.
fn spawn_frame_writer(
    socket: PathBuf,
    frames_on: Arc<AtomicBool>,
    frames: mpsc::Receiver<Vec<u8>>,
) {
    let _ = std::thread::Builder::new()
        .name("tuta-frames-send".into())
        .spawn(move || {
            let mut stream: Option<UnixStream> = None;
            loop {
                if !frames_on.load(Ordering::Relaxed) {
                    // Frames produced while closed are stale; drop them and
                    // the connection.
                    while frames.try_recv().is_ok() {}
                    stream = None;
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                if stream.is_none() {
                    match UnixStream::connect(&socket) {
                        Ok(connected) => {
                            let _ = connected.set_write_timeout(Some(Duration::from_secs(2)));
                            stream = Some(connected);
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(250));
                            continue;
                        }
                    }
                }
                // The timer only queues frames while frames are on, so this
                // waits quietly when the tab closes again.
                let Ok(png) = frames.recv() else {
                    break;
                };
                if let Some(connected) = stream.as_mut() {
                    let length = (png.len() as u32).to_be_bytes();
                    if connected
                        .write_all(&length)
                        .and_then(|()| connected.write_all(&png))
                        .is_err()
                    {
                        stream = None;
                    }
                }
            }
        });
}

/// Write one command line to a child socket. `false` means nothing is
/// listening there.
fn send(socket: &Path, message: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));
    stream.write_all(message.as_bytes()).is_ok()
}

/// Retry `send` while a just-started child binds its socket.
fn send_until(socket: &Path, message: &str, attempts: u32) -> bool {
    for _ in 0..attempts {
        if send(socket, message) {
            return true;
        }
        std::thread::sleep(FOCUS_RETRY);
    }
    false
}

/// Whether a child is listening on its focus socket.
fn is_listening(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// When this app last asked for a child, so a second request cannot spawn one
/// while the first is still starting.
fn last_spawn() -> &'static Mutex<Option<Instant>> {
    static LAST_SPAWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_SPAWN.get_or_init(|| Mutex::new(None))
}

/// Remove a socket file only when it is a socket: a file of the user's own in
/// its place must never be deleted.
fn remove_our_socket(socket: &Path) {
    if fs::symlink_metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket()) {
        let _ = fs::remove_file(socket);
    }
}

/// One window per profile, with the same flock the mail profile uses.
struct InstanceLock(#[allow(dead_code)] File);

impl InstanceLock {
    fn lock_file(data_dir: &Path) -> io::Result<File> {
        let directory = data_dir.join("tuta");
        fs::create_dir_all(&directory)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("webview.lock"))
    }

    /// `None` when another window holds it.
    fn try_acquire(data_dir: &Path) -> io::Result<Option<Self>> {
        let file = Self::lock_file(data_dir)?;
        // SAFETY: `file` is an open descriptor owned by this function; flock
        // only changes its lock state, held until the process exits.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::EWOULDBLOCK) => Ok(None),
                _ => Err(error),
            };
        }
        Ok(Some(Self(file)))
    }

    /// Waits for the window that owns the lock to exit (only used by a child
    /// racing its parent's launch).
    fn acquire(data_dir: &Path) -> io::Result<Self> {
        let file = Self::lock_file(data_dir)?;
        // SAFETY: as above; a blocking flock on our own descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}

/// The child's end of the focus socket.
struct FocusListener {
    listener: UnixListener,
    path: PathBuf,
}

impl FocusListener {
    fn bind(path: &Path) -> io::Result<Self> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let listener = match UnixListener::bind(path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "a Tuta window is already serving this profile",
                    ));
                }
                remove_our_socket(path);
                UnixListener::bind(path)?
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            listener,
            path: path.to_owned(),
        })
    }

    /// Answers in a thread; `EventLoopProxy` is the only thread-safe way into
    /// the GTK event loop, which owns the window.
    fn serve(self, proxy: EventLoopProxy<Wake>) {
        let _ = std::thread::Builder::new()
            .name("tuta-focus".into())
            .spawn(move || {
                for stream in self.listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(SOCKET_TIMEOUT));
                    let mut request = [0u8; 32];
                    let Ok(count) = stream.read(&mut request) else {
                        continue;
                    };
                    let wake = match String::from_utf8_lossy(&request[..count]).trim() {
                        "show" => Wake::Show,
                        "hide" => Wake::Hide,
                        "frames on" => Wake::Frames(true),
                        "frames off" => Wake::Frames(false),
                        "quit" => Wake::Quit,
                        _ => continue,
                    };
                    if proxy.send_event(wake).is_err() {
                        break;
                    }
                }
                remove_our_socket(&self.path);
            });
    }
}
