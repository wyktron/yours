# Literata

This directory contains the unmodified Literata static TrueType faces used for
the reading pane body.

- Family: `Literata`
- Upstream files: `Literata-Regular.ttf` (400), `Literata-SemiBold.ttf` (600)
- Source: <https://fonts.google.com/specimen/Literata>
- Retrieved: 2026-11-25 from Google Fonts through the Google Fonts CSS API
- SHA-256:
  - `Literata-Regular.ttf`: `1d6eafdc620333fcc3d573c115fc62ea78d0965d1f72a8f549fc9660fef556b3`
  - `Literata-SemiBold.ttf`: `1bc8575bc9f4b7ed93f594924f6639e58d78dbc9bc10491414c4684f69264b92`
- License: SIL Open Font License 1.1; see [OFL.txt](OFL.txt)

The faces are embedded through `ui/app.slint` for the reader's plain-text view
and registered directly with the email renderer's Fontique collection, so a
message without an authored family reads in Literata regardless of installed
system fonts. The existing Arial/Liberation/Noto fallback chain remains behind
it, and sender-authored font families still take precedence.
