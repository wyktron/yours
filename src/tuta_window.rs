//! The embedded Tuta web app.
//!
//! Tuta has no IMAP endpoint and no local bridge: its account exists only in
//! the web app at <https://app.tuta.com>. Nextcloud shows Tuta the same way,
//! by embedding that web app, so Yours does too, in a window of its own. The
//! sidebar entry is labelled "Tuta web app" because it is the web app, not a
//! mail account inside Yours.
//!
//! # Why a separate process
//!
//! Slint's winit backend owns the application's event loop on the main
//! thread. The GTK/WebKitGTK stack that paints the web view requires that
//! same main thread: `gtk::init` and tao's event loop both assert it, and two
//! event loops cannot share one thread. The window therefore runs as
//! `yours --tuta-webview <data dir>`, a child of the app that owns the main
//! thread of its own process. The child is detached: closing the Tuta window
//! leaves Yours running, and Yours exiting leaves Tuta open. The sidebar
//! button focuses the window when it is already open, over a private socket.
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
    time::Duration,
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
use webkit2gtk::{NotificationExt, WebViewExt};
use wry::{
    PermissionKind, PermissionResponse, WebContext, WebViewBuilder, WebViewBuilderExtUnix,
    WebViewExtUnix,
};

/// The argument that turns this executable into the Tuta web app window.
const ARG: &str = "--tuta-webview";
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
/// How long the button waits for a starting window to answer its socket.
const FOCUS_ATTEMPTS: u32 = 60;
const FOCUS_RETRY: Duration = Duration::from_millis(50);
const SOCKET_TIMEOUT: Duration = Duration::from_millis(500);

/// Events the focus socket delivers to the window's event loop.
enum Wake {
    /// Bring the window to the front.
    Show,
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

/// Wire the sidebar entry: the window is available here and opens on click.
pub fn install(app: &crate::AppWindow, data_dir: &Path) {
    // The window's notifications are handed back here, where the bridge, the
    // settings store and the speech engine live. A listener that cannot start
    // costs only that speech, never the window.
    let notifications = notify_socket_path(data_dir);
    let _ = std::thread::Builder::new()
        .name("tuta-notify".into())
        .spawn(move || serve_notifications(&notifications));
    let data_dir = data_dir.to_owned();
    let global = app.global::<crate::TutaWebApp>();
    global.set_available(true);
    global.on_open(move || {
        if let Err(error) = open(&data_dir) {
            eprintln!("tuta: could not open the web app: {error}");
        }
    });
}

/// Open the Tuta window, or bring the open one to the front.
///
/// The child holds `instance.lock` for its whole lifetime, so two clicks
/// cannot start two windows and a leftover file never blocks a launch.
fn open(data_dir: &Path) -> io::Result<()> {
    let socket = socket_path(data_dir);
    match InstanceLock::try_acquire(data_dir)? {
        // A window owns the lock: ask it to show, waiting out its start.
        None => {
            for _ in 0..FOCUS_ATTEMPTS {
                if focus(&socket) {
                    return Ok(());
                }
                std::thread::sleep(FOCUS_RETRY);
            }
            Ok(())
        }
        Some(lock) => {
            // The lock proves no window is serving this profile; a socket
            // file without a listener is debris from a window that was killed.
            remove_our_socket(&socket);
            let executable = std::env::current_exe()?;
            Command::new(executable)
                .arg(ARG)
                .arg(data_dir)
                // The window has no console; report nothing through ours.
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            drop(lock);
            Ok(())
        }
    }
}

/// The child process: one window, one WebKitGTK web view, one event loop.
fn run() -> Result<(), Box<dyn Error>> {
    // The parent passes its profile so the web app follows the same data
    // directory as the mail it belongs to; a manual launch falls back to the
    // default profile.
    let data_dir = match std::env::args_os().nth(2) {
        Some(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => Paths::default_dirs()?.data_dir,
    };
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
            let _ = focus(&socket);
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
        .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
        .with_min_inner_size(LogicalSize::new(480.0, 360.0));
    if let Some(icon) = icon() {
        builder = builder.with_window_icon(Some(icon));
    }
    let window = builder.build(&event_loop)?;
    window.gtk_window().set_icon_name(Some(APP_ID));

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

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Wake::Show) => {
                window.set_visible(true);
                window.set_focus();
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::WindowEvent {
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

/// The socket the sidebar uses to bring the window to the front, or the one
/// the window uses to hand a notification to the app; both live in the private
/// per-user runtime directory, so they disappear at logout.
fn socket_path(data_dir: &Path) -> PathBuf {
    runtime_socket(data_dir, "focus", "webview.sock")
}

fn notify_socket_path(data_dir: &Path) -> PathBuf {
    runtime_socket(data_dir, "notify", "notify.sock")
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

/// Ask a running window to show itself. `false` means it is not listening.
fn focus(socket: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));
    stream.write_all(b"show\n").is_ok()
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
                    let mut request = [0u8; 16];
                    let Ok(count) = stream.read(&mut request) else {
                        continue;
                    };
                    if request[..count].starts_with(b"show") && proxy.send_event(Wake::Show).is_err()
                    {
                        break;
                    }
                }
                remove_our_socket(&self.path);
            });
    }
}
