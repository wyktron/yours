# The Tuta web app in Yours

Tuta has no IMAP endpoint and no local bridge, so its account exists only in
the official web app at <https://app.tuta.com>. Yours runs that web app in a
child process of its own (`yours --tuta-webview <data dir>`, see
`src/tuta_window.rs`), labelled "Tuta web app" because it is the web app, not
a mail account inside Yours.

## It runs in the background

On a first run the sidebar shows the interactive window, because a snapshot
cannot be logged into. Once that window has been on screen, the child starts
with Yours at every launch, with its window never mapped
(`<data dir>/tuta/webview-shown` marks the first use).
The page is still loaded and running there: it keeps its Tuta session (cookies,
localStorage and IndexedDB under `<data dir>/tuta/webview`) and keeps receiving
new mail, so its notifications are read aloud even while no window is on
screen. The window is created realized but hidden rather than created on
demand, because WebKitGTK can only paint and notify from a realized view.

Closing the Tuta window hides it; it does not end the process. The sidebar's
"Tuta web app" row opens the tab, and the tab's **Open window** button brings
the interactive window back. Quitting Yours ends the child with it: the child
watches its parent and terminates when Yours' main thread exits, so no orphan
web app keeps holding the Tuta session.

## The tab is a live picture, not an embedded window

The owner asked for Tuta inside the Yours window. The shipped tab is the web
view's frames through WebKitGTK's snapshot API
(`webkit_web_view_get_snapshot`), refreshed about once a second while the tab
is open. It is **not interactive**: clicking and typing need the window itself,
which the **Open window** button shows. The tab says so, in a banner above the
picture.

Forwarding clicks from the picture is possible in principle (map a tab click
to a window coordinate and synthesize a GTK button event at it), but it would
not make the tab usable: WebKit needs the GTK window to set keyboard focus, and
scrolling, text selection, drag and drop, clipboard, IME and keyboard input all
depend on that focus. Shipping that half-interaction would invite the owner to
try to type into a picture, so the tab stays read-only and the window is the
one place that takes input.

This is forced by the platform, not a shortcut:

- Yours runs on Wayland (Slint's winit backend, `wayland-1` here). A Wayland
  client cannot reparent another client's surface into its own window: there is
  no cross-client subsurface, and `xdg-foreign` exports a surface for transient
  parents, not for embedding one client into another's content area.
- `XReparentWindow` would do it on X11, and Yours would have to run as an X11
  client for its window to be an embedding parent; the native session here is
  Wayland, so there is no X11 parent to attach to.
- WebKitGTK has no offscreen render API for a `WebKitWebView`; the snapshot API
  is the only way to get the page as an image. A GTK widget cannot be placed in
  the Slint window either: GTK/WebKitGTK require the process's main thread and a
  GtkWindow they own, and Slint's winit backend exposes no native parent handle
  a foreign widget could attach to.
- A layer-shell surface aligned under the tab bar would float above every normal
  window rather than be clipped inside Yours, so it cannot pass for a tab.

If Yours ever runs on X11, `XReparentWindow` is the route to a real embedded
tab; the child already has a private socket and a per-profile window, so the
window side is ready for it.

## Why the tab exists at all

The owner asked for Tuta in a tab rather than a separate window. The tab keeps
the web app's place inside Yours, shows at a glance that it is running in the
background, and offers the one action the picture cannot: opening the window to
interact. Notifications do not depend on the tab being open — the child is
started at Yours' startup and reads its session whether the tab is showing or
not.
