#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The launch clock starts before anything else, so the phase markers in
    // `run` cover dynamic loading, platform setup and the window's first frame.
    flectar_mail::mark_process_start();
    #[cfg(glibc)]
    tune_allocator();
    // Slint 1.19 keeps the native window and its rendering surface when
    // hidden on X11 and Windows. Use its window recreation path for tray mode,
    // as it already does on Wayland, so a hidden window holds no surface.
    // macOS keeps its window: AppKit attributes set after creation would be
    // lost on recreation.
    #[cfg(any(linux_desktop, target_os = "windows"))]
    if std::env::var_os("SLINT_DESTROY_WINDOW_ON_HIDE").is_none() {
        // SAFETY: the desktop entry point has not started any threads or
        // initialized libraries yet. Never mutate this from a tray callback.
        unsafe { std::env::set_var("SLINT_DESTROY_WINDOW_ON_HIDE", "1") };
    }
    // The Tuta tab is a real, interactive page only when Yours is an X11
    // client: with XWayland available the app reparents the web app's window
    // into its own. Prefer X11 there; YOURS_TUTA_X11=0 keeps Wayland and the
    // snapshot tab. This runs before the backend picks its window system.
    #[cfg(linux_desktop)]
    flectar_mail::tuta_embed::prefer_x11_for_tuta();
    // SAFETY: as above, no other thread exists yet.
    #[cfg(desktop)]
    unsafe {
        flectar_mail::take_launch_activation_token()
    };
    #[cfg(desktop)]
    if let Some(code) = flectar_mail::pdf_preview::run_worker_if_requested() {
        std::process::exit(code);
    }
    #[cfg(desktop)]
    if let Some(code) = flectar_mail::agents::run_bridge_if_requested() {
        std::process::exit(code);
    }
    #[cfg(linux_desktop)]
    if let Some(code) = flectar_mail::spelling::run_index_worker_if_requested() {
        std::process::exit(code);
    }
    // The Tuta web app runs in a process of its own: its GTK/WebKitGTK event
    // loop needs the main thread, which Slint's event loop owns here. See
    // src/tuta_window.rs for the whole story.
    #[cfg(linux_desktop)]
    if let Some(code) = flectar_mail::tuta_window::run_if_requested() {
        std::process::exit(code);
    }
    flectar_mail::run_desktop(flectar_mail::platform::launch_context()?)
}

/// The mail shell has a small number of worker threads. One glibc allocation
/// arena is enough and prevents per-thread arenas from retaining render/sync
/// scratch memory after a burst of work. Set in-process rather than through
/// the environment, which the browser and other launched programs would
/// inherit. Explicit user tuning wins.
#[cfg(glibc)]
fn tune_allocator() {
    let tunables = std::env::var("GLIBC_TUNABLES").unwrap_or_default();
    let user_set = |variable: &str, tunable: &str| {
        std::env::var_os(variable).is_some() || tunables.contains(tunable)
    };
    // SAFETY: mallopt only adjusts allocator parameters; no pointers change.
    unsafe {
        if !user_set("MALLOC_ARENA_MAX", "glibc.malloc.arena_max") {
            libc::mallopt(libc::M_ARENA_MAX, 1);
        }
        if !user_set("MALLOC_TRIM_THRESHOLD_", "glibc.malloc.trim_threshold") {
            libc::mallopt(libc::M_TRIM_THRESHOLD, 64 * 1024);
        }
    }
}
