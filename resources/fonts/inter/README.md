# Inter

This directory contains the unmodified Inter static TrueType faces used for the
Yours interface.

- Family: `Inter`
- Upstream files: `Inter-Regular.ttf` (400), `Inter-Medium.ttf` (500),
  `Inter-SemiBold.ttf` (600)
- Source: <https://fonts.google.com/specimen/Inter>
- Retrieved: 2026-11-25 from Google Fonts through the Google Fonts CSS API
- SHA-256:
  - `Inter-Regular.ttf`: `1b08e7fc267a5c7e1d614100f604b83e7e8a0be241f0f288faa2b3ac93a683ba`
  - `Inter-Medium.ttf`: `8c883f63b2c4157d997319f2c8bc6995ed4357ef371940d31ca159004a4aae63`
  - `Inter-SemiBold.ttf`: `e7a1aaf7eda9f2fad4131725fa556265ec75ca7b2d756260173a040363e8d4f7`
- License: SIL Open Font License 1.1; see [OFL.txt](OFL.txt)

Slint embeds the three weights at compile time through `ui/app.slint` and uses
Inter as the default interface family. System and script fallbacks still cover
glyphs outside Inter's coverage.
