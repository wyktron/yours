# The Tuta web app in Yours

Tuta has no IMAP endpoint and no local bridge, so its account exists only in
the official web app at <https://app.tuta.com>. Yours runs that web app in a
child process of its own (`yours --tuta-webview <data dir>`, see
`src/tuta_window.rs`), labelled "Tuta web app" because it is the web app, not
a mail account inside Yours.

The tab is meant to be the real page, not a picture of it, whenever the session
lets Yours be an X11 client. On a Wayland-only session it falls back to a live
snapshot with an explicit note.

## It runs in the background

The child starts with Yours at every launch with its window never mapped. The
page is still loaded and running there: it keeps its Tuta session (cookies,
localStorage and IndexedDB under `<data dir>/tuta/webview`) and keeps receiving
new mail, so its notifications are read aloud even while no window is on
screen. The window is created realized but hidden rather than created on
demand, because WebKitGTK can only paint and notify from a realized view.

Closing the Tuta tab hides the page; it does not end the process. The sidebar's
"Tuta web app" row opens the tab, or shows the tab that already holds it.
Quitting Yours ends the child with it: the child watches its parent and
terminates when Yours' main thread exits, so no orphan web app keeps holding
the Tuta session.

## The interactive tab: X11 embedding

The owner asked for Tuta in its own tab inside Yours, with clicks landing in
the page. The way to do that is `XReparentWindow`: on a Wayland session with
XWayland, Yours runs as an X11 client (see `prefer_x11_for_tuta` in
`src/tuta_embed.rs`), so its window is an X11 window that another client's
window can be put inside.

1. The app connects to the X server and finds its own window (`_NET_WM_PID`).
2. The child publishes its window id in `<data dir>/tuta/webview.xid` after it
   realizes its window.
3. The app reparents that window into its own at the tab's content rectangle,
   marks it override-redirect so the window manager never sees it as a
   window of its own, and sizes it to the tab.
4. The child maps its window when the tab shows and unmaps it when the tab is
   left; the process and the page keep running either way.
5. The app keeps the window's rectangle equal to the tab's rectangle: the
   Tuta view publishes its content rectangle every 100 ms, and the app moves
   or resizes the embedded window when it changes. Window moves need nothing:
   the child is inside Yours' window and moves with it.
6. The page takes the keyboard. KWin gives X11 focus back to Yours' window
   within a heartbeat of `XSetInputFocus`, so the app focuses the embedded
   window and holds an `XGrabKeyboard` on it while the tab shows, releasing it
   when the tab is left. Clicks, scrolling and selection are the page's own.

The Tuta tab is a tab in the tab strip because it is a `Tuta` tab location
(`src/tabs.rs`): the tab strip can show it beside Mail tabs, close it, reopen
it, and restore it with the other tabs.

## The fallback: a live picture

A Wayland-only session has no X server to attach to, so the tab cannot be the
page. There it stays the web view's frames through WebKitGTK's snapshot API
(`webkit_web_view_get_snapshot`), refreshed about once a second while the tab
is open, with a banner that says it is a picture and the **Open interactive
window** button that shows the window for clicking and typing. The tab name,
the banner and the button are the whole story: the state is not hidden.

This is forced by the platform: a Wayland client cannot reparent another
client's surface into its own window, WebKitGTK has no offscreen render API,
and a GTK widget cannot be placed in the Slint window because GTK/WebKitGTK
need the process's main thread and a GtkWindow they own.

## What is verified, and what is not

Verified on onyx (its KWin/XWayland seat, DISPLAY=:0), with the binary built
from this tree: the reparent itself. The real WebKitGTK window of the web app
sits inside the Yours window at the tab's rectangle, which

    xwininfo -id 0x1600003 -tree
    Window id: 0x1600003 "Yours"   Parent window id: 0x374 (root)
     1 child:
     0x1a00003 "Tuta web app": ("org.yours.mail" "Org.yours.mail")  1500x950+0+0
        1 child: 0x1a00004 (has no name): ()  1x1+-1+-1

shows, produced by exactly the calls `src/tuta_embed.rs` makes
(`XReparentWindow`, configure, map, `XSetInputFocus`, `XGrabKeyboard`). Yours
runs as an X11 client there and logs "tuta: X11 backend, so the Tuta tab can
embed the web app".

Interaction was verified in a prototype spike of the same calls, not yet
through the app's own tab: a WebKit page reparented into another X11 window
rendered inside it, a click handler received the clicks, text typed with
`xdotool` arrived in the page's input, and wheel events reached the page. That
spike used the real webview process of this app, but the reparent was issued by
a test tool rather than by the app's tab code.

**Not verified yet: clicking and typing through the app's own Tuta tab on a
seat where automated input is allowed.** Two things block that on onyx:

- Its KDE seat gates synthetic input: `xdotool` raises a "Remote Control —
  xdotool is asking to control input devices" prompt, and until that is
  answered the clicks and keys never reach the app. `ydotool` (`uinput`) was
  installed and its daemon ran, but its key and click events had no effect on
  the app window either. A real mouse and keyboard are unaffected: the whole
  point of the embed is that the page receives native input events.
- **Known issue: a fresh profile on onyx stalls in startup.** Both backends
  (the X11 default and `YOURS_TUTA_X11=0`) stop after
  `startup-phase: space_frames_complete`, before the block that registers the
  app's lock callbacks and `tuta_window::install`, so the window keeps showing
  the first-run "Set an app password" card, no `--tuta-webview` child is
  started, no `tuta/webview.xid` appears, and the tab cannot be exercised. The
  same tree starts normally on tron, so this is an onyx environment issue, not
  the X11 switch. Do not read the tab as broken from that: the reparent proof
  above used a child started from the same binary.

## Launching

Nothing has to be set by hand for the normal case: on a session with XWayland,
`prefer_x11_for_tuta` drops `WAYLAND_DISPLAY` before Slint picks its backend,
so Yours starts as an X11 client. The X11 backend reads `Xft.dpi`, which the
compositor's scale factor puts in the X resource database, so the window keeps
its physical size. If `XAUTHORITY` is missing from the environment (a service
or bare-shell launch), the cookie in `$XDG_RUNTIME_DIR/xauth_*` is adopted.

Environment overrides:

- `YOURS_TUTA_X11=0` keeps the Wayland backend (and the snapshot tab).
- `WINIT_X11_SCALE_FACTOR=<factor>` overrides the X11 scale factor.
- `YOURS_TUTA_TAB=1` opens the tab at startup, for checking the tab without a
  click.
- `YOURS_TUTA_WEB_APP_URL=<url>` loads another page in the same web view, for
  testing the notification path without a Tuta login.

## Checking it

With Yours running as an X11 client and the Tuta tab open:

    # the app's window, by its pid
    xdotool search --pid $(pgrep -x yours | head -1)
    # the tree proves the page's window is inside Yours' window
    xwininfo -tree -id <parent xid>
    # the page takes clicks and typing
    xdotool mousemove <x> <y> click 1
    xdotool type 'someone@example.com'
    # and scrolling
    xdotool click 5

The embedded window is the child of Yours' window in that tree, and its
geometry is the Tuta tab's content rectangle.

## Why the tab exists at all

The owner asked for Tuta in a tab rather than a separate window. The tab keeps
the web app's place inside Yours, and with X11 embedding it is the page: the
login form, the mailbox and the keyboard behave exactly as in the window.
Notifications do not depend on the tab being open: the child is started at
Yours' startup and reads its session whether the tab is showing or not.
