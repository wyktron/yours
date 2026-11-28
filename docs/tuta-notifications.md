# Tuta notifications

Tuta has no IMAP, so Yours cannot sync it. The Tuta desktop app does show a
system notification for every new message, and each one reaches the session
bus as a method call to `org.freedesktop.Notifications.Notify`. Yours listens
for exactly that call and reads a short line aloud through the same speech
engine, quiet-hours window and settings as its own new-mail announcements.

## What is read aloud

The voice says "You've got email from <sender>." The sender is the
notification's summary when that reads like a name, otherwise the notifying
application's own name. Notification bodies are never read out; a summary is
capped at 120 characters and flattened to one line.

## Which notifications qualify

- The notifying application (or its `desktop-entry` hint) must match one of
  `tutanota`, `tuta` or `tuta-mail`, plus whatever the user adds under
  Settings > Notifications > "Also announce notifications from…". Matching is
  case-insensitive and open-ended, so a Flatpak id like
  `com.tutanota.Tutanota` matches `tutanota`.
- Yours' own notifications are ignored.
- A notification with an empty body is ignored.
- An identical notification (same app, summary and body) inside four seconds
  is read once.

## Settings

- **Announce Tuta notifications** is on by default when a Tuta application is
  detected (a `*.desktop` file naming Tuta, or a Tuta app seen notifying on
  the bus) and off otherwise. Once the user toggles it, the saved choice wins.
- The extra application names field takes a comma-separated list.

## When the bus is unavailable

A headless test run, a Flatpak without the session bus, or a bus that refuses
monitoring leaves the bridge inert. Settings > General > "What this app needs"
then shows **Tuta notifications: not available**, and the log says the same.

## Manual check

With the Tuta desktop app running and Yours running on the same desktop:

```sh
notify-send -a Tuta 'Ana' 'New email'
```

Yours should say "You've got email from Ana." The log line
`notify bridge: read aloud: You've got email from Ana.` appears on stderr, and
a short-lived audio player process (for example `pw-play`) plays a WAV from
the temp directory. Turn the switch off and send the same notification again:
Yours logs `notify bridge: staying silent…` and opens no player.
