//! X11 embedding for the Tuta tab.
//!
//! Tuta exists only as a web app, so Yours keeps it running in a process of
//! its own (src/tuta_window.rs) and shows it inside the window as a tab. On a
//! Wayland session the tab can only be a picture of the page: a client cannot
//! reparent another client's surface into its own.
//!
//! On a Wayland session with XWayland, Yours itself runs as an X11 client
//! (see `prefer_x11_for_tuta` in src/desktop.rs). Then the GTK/WebKitGTK
//! window of the web app is an ordinary X11 window of another client, and
//! `XReparentWindow` puts it inside Yours' own X11 window: the page paints
//! into the tab's rectangle and gets real pointer, wheel and keyboard events.
//! It is the page, not a picture of it.
//!
//! # Who does what
//!
//! This module (the app's process) does all of the X work: it finds Yours'
//! window, learns the child's window id from the file the web app writes
//! (`<data dir>/tuta/webview.xid`), reparents the child, keeps its geometry
//! equal to the tab's rectangle, and takes the keyboard. The child only maps
//! and unmaps its own window when asked, so GTK never has to know it is being
//! embedded and never fights the app for geometry.
//!
//! # The keyboard
//!
//! X11 focus belongs to a window manager's managed window. KWin gives focus
//! back to Yours' window within a heartbeat of `XSetInputFocus` calling for
//! the embedded child, so the page would never see a keystroke. A keyboard
//! grab (`XGrabKeyboard`, owner events) on the child's window freezes the
//! focus there while the tab is showing and is released when it is left;
//! the keys themselves still reach the page because owner events report them
//! to the focus window's client, which is the web app.
//!
//! # Not available
//!
//! Wayland-only sessions have no X server to attach to. `connect` returns
//! `None` and the tab keeps the snapshot path with its explicit note.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, GrabMode,
    InputFocus, MapState, Window,
};

/// Where the web app publishes its X11 window id.
const XID_FILE: &str = "webview.xid";

/// The rectangle the embedded window occupies, in the window's physical
/// pixels: relative to Yours' window's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The app's side of the X11 window: enough to place another client's window
/// inside it.
pub struct Embedder {
    connection: x11rb::rust_connection::RustConnection,
    /// Yours' own top-level X11 window.
    own: Window,
    /// The root window, for absolute coordinates.
    root: Window,
    /// The web app's window once it is known.
    child: Cell<Option<Window>>,
    /// Whether the keyboard grab is held.
    grabbed: Cell<bool>,
    /// Whether a request has already failed, so a broken X setup is reported
    /// once instead of on every tick.
    complained: Cell<bool>,
}

impl Embedder {
    /// Connect to the X server and find Yours' own window. `None` on a
    /// Wayland-only session, or whenever Yours is not an X11 client.
    pub fn connect() -> Option<Self> {
        let (connection, screen_number) = x11rb::connect(None).ok()?;
        let screen = &connection.setup().roots[screen_number];
        let root = screen.root;
        let pid = std::process::id();
        let pid_atom = connection
            .intern_atom(false, b"_NET_WM_PID")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let children = connection.query_tree(root).ok()?.reply().ok()?.children;
        let mut own = None;
        let mut unmapped = None;
        for window in children {
            // A window manager does not manage its own frames, and another
            // client's window (a Tuta web app left over from an earlier run)
            // carries another pid; only Yours' own toplevel answers. Windows
            // come and go while this scans, so one that cannot be read is
            // skipped rather than ending the search.
            let attributes = match connection
                .get_window_attributes(window)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
            {
                Some(attributes) => attributes,
                None => continue,
            };
            if attributes.override_redirect
                || window_pid(&connection, window, pid_atom) != Some(pid)
            {
                continue;
            }
            if attributes.map_state == MapState::VIEWABLE {
                own = Some(window);
                break;
            }
            // The window exists but is not mapped yet: the tab can still be
            // placed inside it, which is what a tray-mode start needs.
            unmapped = unmapped.or(Some(window));
        }
        let own = own.or(unmapped);
        Some(Self {
            connection,
            own: own?,
            root,
            child: Cell::new(None),
            grabbed: Cell::new(false),
            complained: Cell::new(false),
        })
    }

    /// Yours' own window, the reparent destination.
    pub fn own(&self) -> Window {
        self.own
    }

    /// The web app's window id, read from the file it publishes. The window
    /// must still exist: a stale line from a child that has since exited never
    /// becomes a reparent target.
    pub fn child_window(&self, data_dir: &Path) -> Option<Window> {
        let text = std::fs::read_to_string(xid_path(data_dir)).ok()?;
        let window = text.trim().parse::<u32>().ok()?;
        if self.child.get() == Some(window) && self.valid(window) {
            return Some(window);
        }
        if !self.valid(window) {
            return None;
        }
        self.child.set(Some(window));
        Some(window)
    }

    /// Whether `window` is a window this connection can reparent.
    fn valid(&self, window: Window) -> bool {
        self.connection
            .get_window_attributes(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some()
    }

    /// Put the web app's window inside Yours' window at `rect` and size it.
    /// The window may be unmapped; the web app maps it when it is shown.
    pub fn reparent(&self, window: Window, rect: Rect) -> bool {
        // The window manager must never see this window as a toplevel of its
        // own, so it is marked override-redirect before it is mapped.
        let marked = self
            .connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().override_redirect(1),
            )
            .map(|cookie| cookie.check().is_ok())
            .unwrap_or(false);
        let moved = self
            .connection
            .reparent_window(window, self.own, clamp_i16(rect.x), clamp_i16(rect.y))
            .map(|cookie| cookie.check().is_ok())
            .unwrap_or(false);
        let sized = self.place(window, rect);
        let _ = self.connection.flush();
        let ok = marked && moved && sized;
        self.complain(ok, "cannot embed the Tuta web app window");
        ok
    }

    /// Move and resize the embedded window to `rect`.
    pub fn place(&self, window: Window, rect: Rect) -> bool {
        let configured = self
            .connection
            .configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x(rect.x)
                    .y(rect.y)
                    .width(rect.width)
                    .height(rect.height),
            )
            .map(|cookie| cookie.check().is_ok())
            .unwrap_or(false);
        let _ = self.connection.flush();
        self.complain(configured, "cannot place the Tuta web app window");
        configured
    }

    /// Whether the embedded window is on screen (mapped and viewable).
    pub fn mapped(&self, window: Window) -> bool {
        self.connection
            .get_window_attributes(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|attributes| attributes.map_state == MapState::VIEWABLE)
    }

    /// The child's rectangle as the X server has it, relative to Yours'
    /// window. The app uses this to notice a toolkit that resized its own
    /// window, so the page always fills the tab exactly.
    pub fn child_rect(&self, window: Window) -> Option<Rect> {
        let geometry = self
            .connection
            .get_geometry(window)
            .ok()?
            .reply()
            .ok()?;
        let translated = self
            .connection
            .translate_coordinates(window, self.own, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(Rect {
            x: i32::from(translated.dst_x),
            y: i32::from(translated.dst_y),
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        })
    }

    /// Whether the pointer is over the tab's rectangle, which is when the
    /// page should hold the keyboard.
    pub fn pointer_over(&self, rect: Rect) -> bool {
        let Some(origin) = self
            .connection
            .translate_coordinates(self.own, self.root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            return false;
        };
        let Some(pointer) = self
            .connection
            .query_pointer(self.own)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            return false;
        };
        let x = i32::from(pointer.root_x) - i32::from(origin.dst_x);
        let y = i32::from(pointer.root_y) - i32::from(origin.dst_y);
        x >= rect.x
            && y >= rect.y
            && x < rect.x.saturating_add(i32::try_from(rect.width).unwrap_or(i32::MAX))
            && y < rect.y.saturating_add(i32::try_from(rect.height).unwrap_or(i32::MAX))
    }

    /// Whether Yours' window is the one the window manager made active. A
    /// keyboard grab takes keys from every window on the X server, so it is
    /// only held while this window is the active one.
    pub fn active(&self) -> bool {
        let Some(atom) = self
            .connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.atom)
        else {
            return true;
        };
        self.connection
            .get_property(false, self.root, atom, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().and_then(|mut values| values.next()))
            // A window manager that does not publish the property cannot be
            // asked; the pointer rule below still keeps the grab local.
            .is_none_or(|window| window == self.own)
    }

    /// Give the page the keyboard: focus the child, then hold a keyboard grab
    /// so the window manager cannot take the focus back. The grab is what
    /// makes typing work, because KWin otherwise re-focuses Yours' window.
    pub fn focus_and_grab(&self, window: Window) {
        let focused = self
            .connection
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
            .map(|cookie| cookie.check().is_ok())
            .unwrap_or(false);
        if !self.grabbed.get() || !focused {
            let grabbed = self
                .connection
                .grab_keyboard(
                    true,
                    window,
                    x11rb::CURRENT_TIME,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.status == x11rb::protocol::xproto::GrabStatus::SUCCESS);
            self.grabbed.set(grabbed);
            self.complain(grabbed, "cannot take the keyboard for the Tuta tab");
        }
        let _ = self.connection.flush();
    }

    /// Give the keyboard back to the app's own window.
    pub fn release(&self) {
        if self.grabbed.get() {
            let _ = self.connection.ungrab_keyboard(x11rb::CURRENT_TIME);
            self.grabbed.set(false);
        }
        let _ = self
            .connection
            .set_input_focus(InputFocus::PARENT, self.own, x11rb::CURRENT_TIME);
        let _ = self.connection.flush();
    }

    /// Take the embedded window off the screen without moving it out of the
    /// tab: the web app hides its own window too, this is the belt.
    pub fn unmap(&self, window: Window) {
        let _ = self.connection.unmap_window(window);
        let _ = self.connection.flush();
    }

    /// Report a failed request once, rather than on every tick.
    fn complain(&self, ok: bool, message: &str) {
        if !ok && !self.complained.replace(true) {
            eprintln!("tuta: {message}");
        }
    }
}

/// The web app's window id file, next to its lock and its WebKit profile.
pub fn xid_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tuta").join(XID_FILE)
}

/// Publish the web app's own window id for the app to pick up. Called after
/// the window is realized and again whenever it is shown, because that is the
/// moment the id is guaranteed to be final.
pub fn publish_xid(data_dir: &Path, window: u32) {
    let path = xid_path(data_dir);
    if let Some(directory) = path.parent()
        && std::fs::create_dir_all(directory).is_err()
    {
        return;
    }
    let _ = std::fs::write(path, format!("{window}\n"));
}

/// Forget a published id, so a child that is restarting cannot be embedded
/// at the id its predecessor held.
pub fn forget_xid(data_dir: &Path) {
    let path = xid_path(data_dir);
    if std::fs::symlink_metadata(&path).is_ok() {
        let _ = std::fs::remove_file(path);
    }
}

/// Point `XAUTHORITY` at the session's XWayland cookie when the environment
/// came without it (a service launch, a bare shell). Returns whether an X
/// connection is worth trying again.
///
/// KWin starts XWayland with a per-session authority file in the runtime
/// directory. The desktop session exports it, but a process started from a
/// bare shell does not have it, and then the X server refuses the connection.
fn adopt_session_x_authority() -> bool {
    if std::env::var_os("XAUTHORITY").is_some() {
        return false;
    }
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(runtime) else {
        return false;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("xauth_") {
            continue;
        }
        // SAFETY: as in `prefer_x11_for_tuta`: before any thread that reads
        // the environment is started.
        unsafe {
            std::env::set_var("XAUTHORITY", entry.path());
        }
        return x11rb::connect(None).is_ok();
    }
    false
}

/// A `_NET_WM_PID` value, which every toolkit sets on its toplevel windows.
fn window_pid(connection: &impl Connection, window: Window, atom: u32) -> Option<u32> {
    connection
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
}

/// X11 window coordinates are signed 16-bit.
fn clamp_i16(value: i32) -> i16 {
    i16::try_from(value.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

/// Run Yours as an X11 client when the session has XWayland.
///
/// The Tuta tab embeds the web app's window by reparenting it into Yours' own
/// X11 window, and only an X11 client has one. On a Wayland session with
/// XWayland the app therefore switches to winit's X11 backend before the
/// backend is chosen; a Wayland-only session is left alone and keeps the
/// snapshot tab. `YOURS_TUTA_X11=0` keeps the Wayland backend on purpose.
///
/// The scale factor is not forced: winit's X11 backend reads the session's
/// `Xft.dpi`, which is what the compositor's scale factor puts in the X
/// resource database, so the window keeps its physical size.
pub fn prefer_x11_for_tuta() {
    if std::env::var("YOURS_TUTA_X11").ok().as_deref() == Some("0") {
        return;
    }
    // Already an X11 client (no Wayland to leave), or no X server at all.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    if x11rb::connect(None).is_err() && !adopt_session_x_authority() {
        return;
    }
    // SAFETY: the launcher calls this before any thread, event loop or
    // library that reads the environment is started, and nothing else mutates
    // the environment concurrently.
    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    eprintln!("tuta: X11 backend, so the Tuta tab can embed the web app");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_stay_inside_the_x11_window_coordinate_range() {
        assert_eq!(clamp_i16(0), 0);
        assert_eq!(clamp_i16(-40), -40);
        assert_eq!(clamp_i16(i32::MAX), i16::MAX);
        assert_eq!(clamp_i16(i32::MIN), i16::MIN);
    }

    #[test]
    fn the_window_id_file_lives_beside_the_web_apps_profile() {
        assert_eq!(
            xid_path(Path::new("/data")),
            Path::new("/data/tuta/webview.xid").to_path_buf()
        );
    }
}
