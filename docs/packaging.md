# Packaging Yours

Yours ships three distro-facing packages. **Flatpak is the primary one**: a
single bundle runs on every distribution with Flatpak, carries its own
Freedesktop runtime and Rust toolchain, and does not depend on the distro's
Rust version. The RPM spec and Debian skeleton build the same sources natively
for distributions that prefer their own package manager.

| Path | Artifact |
| --- | --- |
| `packaging/flatpak/org.yours.mail.yaml` | Flatpak manifest (primary) |
| `packaging/flatpak/cargo-sources.json` | Vendored crates generated from `Cargo.lock` |
| `packaging/rpm/yours.spec` | RPM source package |
| `packaging/debian/` | Debian source package skeleton |
| `docs/packaging.md` | This file |

The release pipeline in `scripts/build-*.sh` produces differently named
artifacts (`flectar-mail.rpm`, `flectar-mail.deb`, test Flatpak bundles) from
already-built executables. The files here are for packagers who build from
source.

## Flatpak

The manifest uses `org.freedesktop.Platform`/`Sdk` 25.08 with the
`org.freedesktop.Sdk.Extension.rust-stable` extension, and builds offline from
`cargo-sources.json`. `finish-args` grant only what the app needs: Wayland
with an X11 fallback, shared IPC, host network (mail sync, OAuth, and the local
Reacher service), DRM for the GPU renderer, `org.freedesktop.secrets` for the
keyring, and `com.canonical.Unity` for the unread badge. Notifications,
browser and file picking use portals and need no extra permission.

Install the tools and runtimes once:

```bash
# Fedora: sudo dnf install flatpak flatpak-builder
# Debian/Ubuntu: sudo apt install flatpak flatpak-builder
flatpak remote-add --if-not-exists --user flathub \
  https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub \
  org.freedesktop.Platform//25.08 \
  org.freedesktop.Sdk//25.08 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08
```

Build and install the bundle. The bundle is named after the app —
`yours.flatpak`, not `flectar-mail.flatpak`:

```bash
# flatpak-builder is not always installed on the host; the Flathub Builder
# app can run it and still reads and writes the checkout's .flatpak-builder
# state directory.
flatpak run --command=flatpak-builder org.flatpak.Builder \
  --user --force-clean --disable-rofiles-fuse --assumeyes \
  --repo=target/yp/repo target/yp/build \
  packaging/flatpak/org.yours.mail.yaml
flatpak build-bundle \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  target/yp/repo target/yp/yours.flatpak org.yours.mail
flatpak install --user -y --reinstall target/yp/yours.flatpak
flatpak run org.yours.mail
```

`scripts/build-flatpak.sh` is the older helper: it calls `flatpak-builder`
from the host's PATH (so the Builder-app command above is needed on hosts
without it) and writes `target/flatpak/flectar-mail.flatpak`; the manual
command above is the path used for the current artifacts.

The manifest's Rust build is single-job on purpose: peak memory for the app
crate is high. `--disable-rofiles-fuse` is optional but lets the build run on
systems without `rofiles-fuse`.

Refresh the vendored crates whenever `Cargo.lock` changes. The generator
writes the same file layout the build expects; CI (`scripts/test-release.py`)
checks that every locked registry crate has an archive and checksum:

```bash
flatpak-cargo-generator -d Cargo.lock \
  -o packaging/flatpak/cargo-sources.json
```

### The embedded Tuta web app and the Flatpak runtime

The sidebar's "Tuta web app" entry embeds <https://app.tuta.com> in a WebKitGTK
web view (`src/tuta_window.rs`). GTK 3 and WebKitGTK 4.1 are therefore
link-time dependencies of the `yours` executable, and the current manifest
needs attention before a bundle will work:

- **`org.freedesktop.Platform` does not ship WebKitGTK.** The manifest as
  written cannot build or run the embedded window. The smaller of the two
  fixes is to switch `runtime`/`sdk` to `org.gnome.Platform`/`
  org.gnome.Sdk` 25.08, whose runtime includes `webkit2gtk-4.1`; the
  alternative is a bundled WebKitGTK module, which is a large build. Neither
  change is made here.
- `cargo-sources.json` must be regenerated with the command above: the
  lockfile now includes `wry`, `tao` and the gtk-rs/WebKitGTK stack, and the
  flatpak build is offline.
- The build image needs GTK/WebKit development headers. The GNOME SDK has
  them; the freedesktop SDK does not.
- No `finish-args` change is needed: the manifest already has Wayland with an
  X11 fallback and host network access for `app.tuta.com`, and WebKitGTK
  delivers the page's notifications through the host's
  `org.freedesktop.portal.Notification`, which every Flatpak may use.
- The Tuta session is kept with the app's data, under
  `~/.var/app/org.yours.mail/data/yours-mail/tuta/webview`, so it is covered
  by the existing data directory and needs no new `--filesystem` grant.

## RPM

`packaging/rpm/yours.spec` builds the workspace with
`cargo build --release --locked --bin yours` (default features: remote
content and the GPU renderer; native packages keep the tray and talk to the
desktop's notification server directly). It installs the binary, desktop
entry, AppStream metadata, hicolor icons, and the dual-license tree.

Beyond libc the executable links `libdbus-1`, GTK 3 and WebKitGTK 4.1: the
last two are the embedded Tuta web app (`src/tuta_window.rs`). Fontconfig,
FreeType, Wayland, X11/XCB and xkbcommon are loaded at runtime, so the spec
requires them explicitly; `gtk3` and `webkit2gtk4.1` are named there too for
clarity even though RPM derives the soname dependencies. `reqwest` uses
rustls, so OpenSSL is not a dependency. Fedora 43+ is the tested family;
`ExclusiveArch` is x86_64 and aarch64.

```bash
git archive --prefix=yours-0.1.0-alpha.7/ \
  -o ~/rpmbuild/SOURCES/yours-0.1.0-alpha.7.tar.gz HEAD
PATH="$HOME/.cargo/bin:$PATH" CARGO_BUILD_JOBS=8 \
  rpmbuild -ba --nodeps packaging/rpm/yours.spec
cp ~/rpmbuild/RPMS/x86_64/yours-0.1.0~alpha.7-1.fc45.x86_64.rpm \
  target/packages/
```

`sudo dnf builddep packaging/rpm/yours.spec` installs the build requirements.
`--nodeps` skips the `BuildRequires` check for the packaging-run smoke
build on a host that does not have the RPMs named in the spec: the 2026-11-28
build used the rustup toolchain (cargo/rust 1.100.0) instead of the `cargo`
and `rust` RPMs, and `libXcursor-devel`, `libxkbcommon-x11-devel` and
`wayland-devel` were absent. Those libraries are only `dlopen`ed at runtime,
not linked, so the build still succeeds; a proper Fedora build host should
install the full `BuildRequires` set and drop `--nodeps`.

The build needs crates.io access unless the tree is vendored (see the comment
at the top of the spec), and needs Rust 1.95 or newer. PDF preview loads
`libpdfium.so` from `%{_libdir}/flectar-mail` if a distribution package
provides it; the spec does not ship the prebuilt PDFium archive.

Install/run/uninstall smoke test (host-local RPM, unsigned, dependencies not
resolved because Fedora has no `yours` repository):

```bash
sudo rpm -Uvh --nodeps --nosignature \
  target/packages/yours-0.1.0~alpha.7-1.fc45.x86_64.rpm
/usr/bin/yours     # run it once; use a throwaway HOME for the smoke test
sudo rpm -e yours
```

## Debian

`packaging/debian/` is a debhelper 13 skeleton: `control`, `rules`,
`install`, `changelog`, `copyright` and `source/format`. It uses the same
release build and installs the same metadata as the RPM.

Debian expects `debian/` at the source root, so build from a checkout with a
temporary link:

```bash
ln -s packaging/debian debian
sudo apt build-dep .          # or install the Build-Depends by hand
dpkg-buildpackage -b -us -uc
rm debian
sudo apt install ../yours_0.1.0~alpha.7-1_*.deb
```

The Debian `Build-Depends` also need `libgtk-3-dev` and
`libwebkit2gtk-4.1-dev` for the embedded Tuta web app; `${shlibs:Depends}`
adds the matching runtime libraries to `Depends`, so no explicit
`libgtk-3-0`/`libwebkit2gtk-4.1-0` entry is needed there.

`dpkg-buildpackage -b` builds the binary package without an orig tarball.
A source upload needs `yours_0.1.0~alpha.7.orig.tar.*` because the format is
`3.0 (quilt)`. Rust 1.95 or newer is required; distributions with an older
`rustc` must build with a newer toolchain. PDFium is handled as in the RPM.

On a Fedora host there is no `dpkg-buildpackage` or `dpkg-deb`, so the
binary package is assembled with Fedora's `dpkg` RPM extracted into the
checkout, without root:

```bash
dnf download dpkg --destdir target/yp/tools/rpms
mkdir -p target/yp/tools/dpkg
( cd target/yp/tools/dpkg && \
  rpm2cpio ../rpms/dpkg-1.23.11-2.fc45.x86_64.rpm | cpio -idm )

# HEAD source export; reuse the release binary built for the RPM
rm -rf /tmp/yp-src && mkdir -p /tmp/yp-src
git archive HEAD | tar -x -C /tmp/yp-src
mkdir -p /tmp/yp-src/target/release
cp target/yp/rpm-extract/usr/bin/yours /tmp/yp-src/target/release/yours
```

`target/yp/rpm-extract` comes from
`rpm2cpio target/packages/yours-*.rpm | cpio -idm` in that directory.
`packaging/debian/build-deb.sh` stages the files exactly as
`packaging/debian/rules` and `packaging/debian/install` do, writes
`DEBIAN/control`, `DEBIAN/md5sums` and the debhelper files (`copyright`,
gzipped `changelog.Debian.gz`), and calls `dpkg-deb --build`:

```bash
./packaging/debian/build-deb.sh /tmp/yp-src target/packages
# equivalently, if dpkg-deb is on PATH and the staging tree already exists:
# dpkg-deb --build --root-owner-group target/yp/deb-root \
#   target/packages/yours_0.1.0~alpha.7-1_amd64.deb
```

The script reads the package name, version and changelog directly from
`packaging/debian/changelog`, and finds `dpkg-deb` on `PATH` or in
`target/yp/tools/dpkg`; `DPKGDEB=/path/to/dpkg-deb` overrides it.

Install/uninstall smoke test on the Fedora host (the `Depends:` names are
Debian names, hence `--force-depends`):

```bash
sudo dnf install -y dpkg
sudo dpkg -i --force-depends \
  target/packages/yours_0.1.0~alpha.7-1_amd64.deb
/usr/bin/yours     # run it once; use a throwaway HOME for the smoke test
sudo dpkg -r yours
sudo dnf remove -y dpkg
rm -rf /var/lib/dpkg   # created by dpkg at install time, not by the RPM
```

## Artifacts built on 2026-11-28

Built from HEAD `bdde9ef` ("settings: pick AI models from the endpoint's
list") on Fedora 45 with rustc/cargo 1.100.0. `target/` is git-ignored, so
these artifacts are not committed.

| Artifact | Size | SHA-256 |
| --- | --- | --- |
| `target/yp/yours.flatpak` | 43,425,952 B (41.4 MiB) | `3ba14f2d0e1f49771da8752b81b2fce7c275b3e50e21512871b006e245c32da2` |
| `target/packages/yours-0.1.0~alpha.7-1.fc45.x86_64.rpm` | 47,771,680 B (45.6 MiB) | `7cc5d1eb59d81ef35e60082a7f190dae67122860f501dda7866803b6d01c90af` |
| `target/packages/yours_0.1.0~alpha.7-1_amd64.deb` | 44,529,296 B (42.5 MiB) | `5e36e9c8bca00488f3d7ab64576e828f09ae8ec4748dbee4efc91a84fe593499` |

The Flatpak app commit is `350df5f3408602496a76243685304d707a39c16e09999e1264f784fc53612acd`;
it replaces the previous local install (`6ea45fd5`, 2026-11-25) in place.

Verification performed for this build:

- **Flatpak** — `flatpak install --user -y --reinstall
  target/yp/yours.flatpak`, then launched under Xvfb with
  `flatpak run --socket=x11 --env=FLECTAR_RENDERER=cpu org.yours.mail`:
  window opened at 1320x800, Settings > About showed
  `Version 0.1.0-alpha.7`, and Settings > General rendered the first card
  "What this app needs" (address checks, AI, speech). The user install was
  left in place: its sandbox data directory is separate from the native
  `~/.local/share/yours-mail` profile, so it does not conflict with a
  native/dev binary.
- **RPM** — `rpm -Uvh --nodeps --nosignature ...`, then `/usr/bin/yours`
  launched once (window shown) with a throwaway HOME, then `rpm -e yours`.
- **DEB** — `dpkg -i --force-depends ...`, then `/usr/bin/yours` launched
  once (window shown) with a throwaway HOME, then `dpkg -r yours`, `dnf
  remove -y dpkg` and removal of the `/var/lib/dpkg` database that
  `dpkg` created.

Headless smoke tests need a throwaway profile: the app asks to set an app
password on first launch. Never set or change the password in the user's own
profile for a packaging check; on the Flatpak side that profile is
`~/.var/app/org.yours.mail/data`.

## Regenerating inputs on each release

The three packages share the commit but not their generated inputs. Refresh
these whenever the tree changes:

- **Version bump** — `Cargo.toml`, `%global upstream_version`/`Version`/
  `%changelog` in `packaging/rpm/yours.spec`, and
  `packaging/debian/changelog` must all name the same version.
- **Vendored crates** — when `Cargo.lock` changes, regenerate
  `packaging/flatpak/cargo-sources.json` with
  `flatpak-cargo-generator -d Cargo.lock -o packaging/flatpak/cargo-sources.json`.
  For an offline native build, run `cargo vendor vendor/`, add the source
  replacement to `.cargo/config.toml` and build with
  `CARGO_NET_OFFLINE=true`; the Flatpak generator output is not a vendor
  directory for rpmbuild.
- **RPM source tarball** — regenerate with
  `git archive --prefix=yours-<version>/ -o ~/rpmbuild/SOURCES/yours-<version>.tar.gz HEAD`;
  the prefix must match `%{name}-%{upstream_version}` used by `%autosetup`.
- **PDFium** — the Flatpak manifest pins the two PDFium archive URLs and
  SHA-256 values from `scripts/stage-pdfium.py`; bump both together. Native
  packages do not ship PDFium and load it from `%{_libdir}/flectar-mail`.
- **Rebuild all three artifacts**, rerun the smoke tests above, and update
  the sizes and SHA-256 values in this section. `scripts/test-release.py`
  checks the manifest/checksums and `scripts/check-release.sh` covers the
  release tarballs.

## Known sandbox caveats

### Data directory and migrations

Flatpak keeps the app private data at
`~/.var/app/org.yours.mail/data/yours-mail` and caches at
`~/.var/app/org.yours.mail/cache/flectar-mail`. A native install uses
`~/.local/share/yours-mail`. Moving between a native package and Flatpak
does not copy it automatically (the sandbox cannot see the host's data
directory). To keep the profile, quit the app and copy it once:

```bash
# native -> Flatpak
mkdir -p ~/.var/app/org.yours.mail/data/yours-mail
cp -a ~/.local/share/yours-mail/. ~/.var/app/org.yours.mail/data/yours-mail/

# Flatpak -> native
mkdir -p ~/.local/share/yours-mail
cp -a ~/.var/app/org.yours.mail/data/yours-mail/. ~/.local/share/yours-mail/
```

The app also renames the old `flectar-mail` profile and keyring namespace to
`yours-mail` on first start; that migration runs inside whichever data root
is active.

### Keyring

Credentials live in the host's freedesktop Secret Service, not in the profile
database. The manifest grants `org.freedesktop.secrets`, so GNOME Keyring,
KeePassXC (Secret Service enabled) and KWallet answer the same collection the
native package uses; passwords survive a switch between native and Flatpak
without being exported. The app copies secrets from the old `flectar-mail`
service name to `yours-mail` the first time they are read. If the sandbox
cannot start or unlock the provider, account setup stops with an explanation
rather than storing a secret insecurely. Check with
`flatpak permissions org.yours.mail` and re-grant with
`flatpak override --user --talk-name=org.freedesktop.secrets org.yours.mail`
if a local policy removed it.

### Optional services

Reacher address checks, the AI features and read-aloud all need services the
packages do not install. **The Flatpak ships only the app**: no Reacher, no
Ollama or model weights, and no speech engine. Because the manifest uses
`--share=network`, 127.0.0.1 is the host's loopback and a Reacher, Ollama or
Kokoro service running on the host is reachable from the sandbox. If the
network namespace is isolated (for example by a user override), loopback
means the sandbox itself and the probe reports the service as unreachable;
point the setting at a reachable host or keep the shared network.

eSpeak NG is not bundled: its installed data is about 25 MB, over the 5 MB
budget, so the Flatpak relies on a host Kokoro server or an online engine for
speech. The app detects a missing service at startup, disables the feature
with an explanation, and never tries to call it anyway.

Per-distribution and Flatpak setup for all three services is in
[optional-services.md](optional-services.md).

### GPG, tray, notifications, file handlers

- The Flatpak runtime provides `gpg`, but the sandbox does not see the host's
  `~/.gnupg` or gpg-agent socket. Existing signing keys must be imported into
  the sandbox, or the user can grant
  `flatpak override --user --filesystem=~/.gnupg:ro \
  --filesystem=xdg-run/gnupg:ro org.yours.mail` before starting the app.
  This exposes private key material and is not in the default manifest.
- The Flatpak has no system-tray icon. Use **Keep running in the background**
  and the desktop's Background Apps list; autostart goes through the Background
  portal.
- Notifications go through `org.freedesktop.portal.Notification`; desktop
  notification servers are not contacted directly in the Flatpak.
- The sandboxed app cannot register itself as the host's default mail handler.
  Do it on the host: `xdg-mime default org.yours.mail.desktop
  x-scheme-handler/mailto`.
- File picking, OAuth browser launches and PDF files chosen for preview use
  portals; arbitrary host paths are not available without a filesystem
  override.
- A standalone `.flatpak` bundle does not update itself. Auto-updates require
  hosting the bundle in an OSTree repository (for example Flathub).

## Checks

```bash
flatpak-builder --show-manifest packaging/flatpak/org.yours.mail.yaml >/dev/null
desktop-file-validate resources/org.yours.mail.desktop
appstreamcli validate --no-net resources/org.yours.mail.metainfo.xml
rpmspec -P packaging/rpm/yours.spec >/dev/null
dpkg-parsechangelog -l packaging/debian/changelog
```
