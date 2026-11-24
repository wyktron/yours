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

Build and install the bundle:

```bash
./scripts/build-flatpak.sh
flatpak install --user target/flatpak/flectar-mail.flatpak
flatpak run org.yours.mail
```

To build by hand instead:

```bash
flatpak-builder --user --force-clean --disable-rofiles-fuse \
  --repo target/flatpak/repo target/flatpak/build \
  packaging/flatpak/org.yours.mail.yaml
flatpak build-bundle \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  target/flatpak/repo target/flatpak/flectar-mail.flatpak org.yours.mail
```

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

## RPM

`packaging/rpm/yours.spec` builds the workspace with
`cargo build --release --locked --bin yours` (default features: remote
content and the GPU renderer; native packages keep the tray and talk to the
desktop's notification server directly). It installs the binary, desktop
entry, AppStream metadata, hicolor icons, and the dual-license tree.

The executable links only `libdbus-1` beyond libc; fontconfig, FreeType,
Wayland, X11/XCB and xkbcommon are loaded at runtime, so the spec requires them
explicitly. `reqwest` uses rustls, so OpenSSL is not a dependency. Fedora 43+
is the tested family; `ExclusiveArch` is x86_64 and aarch64.

```bash
git archive --prefix=yours-0.1.0-alpha.7/ \
  -o ~/rpmbuild/SOURCES/yours-0.1.0-alpha.7.tar.gz HEAD
rpmbuild -ba packaging/rpm/yours.spec
sudo dnf install ~/rpmbuild/RPMS/*/yours-0.1.0~alpha.7-1*.rpm
```

`sudo dnf builddep packaging/rpm/yours.spec` installs the build requirements.
The build needs crates.io access unless the tree is vendored (see the comment
at the top of the spec), and needs Rust 1.95 or newer. PDF preview loads
`libpdfium.so` from `%{_libdir}/flectar-mail` if a distribution package
provides it; the spec does not ship the prebuilt PDFium archive.

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

`dpkg-buildpackage -b` builds the binary package without an orig tarball.
A source upload needs `yours_0.1.0~alpha.7.orig.tar.*` because the format is
`3.0 (quilt)`. Rust 1.95 or newer is required; distributions with an older
`rustc` must build with a newer toolchain. PDFium is handled as in the RPM.

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

### Local Reacher

The optional Reacher address check defaults to `http://127.0.0.1:8084`.
Because the manifest uses `--share=network`, 127.0.0.1 is the host's loopback
and a Reacher service running on the host is reachable from the sandbox. If
the network namespace is isolated (for example by a user override), loopback
means the sandbox itself and checks fail; point the setting at a reachable
host or keep the shared network. Reacher is not bundled with any package.

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
