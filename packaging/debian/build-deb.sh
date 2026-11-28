#!/usr/bin/env bash
# Assemble the Debian binary package with dpkg-deb, without debhelper or
# dpkg-buildpackage. Useful on a non-Debian host that only has the dpkg
# tools (for example Fedora's dpkg RPM extracted under target/yp/tools).
#
# Usage: packaging/debian/build-deb.sh <source-tree> [output-dir]
#
# <source-tree> is a HEAD export containing target/release/yours; the
# resources and license files are taken from that tree, the packaging
# metadata from this checkout. DPKGDEB overrides the dpkg-deb binary.
set -euo pipefail
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
tree=${1:?usage: $0 <source-tree> [output-dir]}
tree=$(CDPATH= cd -- "$tree" && pwd)
out=${2:-$repo/target/packages}
bin=$tree/target/release/yours
root=$repo/target/yp/deb-root
control=$repo/packaging/debian/control
changelog=$repo/packaging/debian/changelog
dpkgdeb=${DPKGDEB:-}
if [ -z "$dpkgdeb" ]; then
  if command -v dpkg-deb >/dev/null 2>&1; then
    dpkgdeb=$(command -v dpkg-deb)
  else
    dpkgdeb=$repo/target/yp/tools/dpkg/usr/bin/dpkg-deb
  fi
fi
for f in "$bin" "$control" "$changelog" "$dpkgdeb"; do
  test -e "$f" || { echo "missing: $f" >&2; exit 1; }
done
test -x "$dpkgdeb" || { echo "not executable: $dpkgdeb" >&2; exit 1; }

pkg=$(sed -n "1s/^\([^ ]*\) .*/\1/p" "$changelog")
version=$(sed -n "1s/^[^ ]* (\([^)]*\)) .*/\1/p" "$changelog")
arch=${ARCH:-amd64}
test -n "$pkg" && test -n "$version" || { echo "cannot read version from $changelog" >&2; exit 1; }

rm -rf "$root"
mkdir -p "$root/DEBIAN"
# Same files as packaging/debian/rules override_dh_auto_install.
install -Dm755 "$bin" "$root/usr/bin/yours"
install -Dm644 "$tree/resources/org.yours.mail.desktop" "$root/usr/share/applications/org.yours.mail.desktop"
install -Dm644 "$tree/resources/org.yours.mail.metainfo.xml" "$root/usr/share/metainfo/org.yours.mail.metainfo.xml"
install -Dm644 "$tree/resources/app-icon/icon-seal-512.png" "$root/usr/share/icons/hicolor/512x512/apps/org.yours.mail.png"
install -Dm644 "$tree/resources/app-icon/icon-seal.svg" "$root/usr/share/icons/hicolor/scalable/apps/org.yours.mail.svg"
install -Dm644 "$tree/LICENSE" "$root/usr/share/doc/$pkg/LICENSE"
install -Dm644 "$tree/THIRD_PARTY_NOTICES.md" "$root/usr/share/doc/$pkg/THIRD_PARTY_NOTICES.md"
cp -a "$tree/LICENSES" "$root/usr/share/doc/$pkg/LICENSES"
install -Dm644 "$tree/resources/fonts/google-sans-flex/OFL.txt" "$root/usr/share/doc/$pkg/google-sans-flex/OFL.txt"
install -Dm644 "$tree/resources/fonts/google-sans-flex/README.md" "$root/usr/share/doc/$pkg/google-sans-flex/README.md"
# debhelper would add these; the hand-built tree adds them explicitly.
install -Dm644 "$repo/packaging/debian/copyright" "$root/usr/share/doc/$pkg/copyright"
gzip -9cn "$changelog" > "$root/usr/share/doc/$pkg/changelog.Debian.gz"
chmod 644 "$root/usr/share/doc/$pkg/changelog.Debian.gz"

installed_size=$(du -sk --exclude=DEBIAN "$root" | cut -f1)
{
  printf "Package: %s\n" "$pkg"
  printf "Version: %s\n" "$version"
  printf "Architecture: %s\n" "$arch"
  printf "Maintainer: Flectar <flectar@users.noreply.github.com>\n"
  printf "Installed-Size: %s\n" "$installed_size"
  printf "Depends: fontconfig, hicolor-icon-theme, libfreetype6, libwayland-client0, libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6, libxkbcommon0, libxkbcommon-x11-0, xdg-utils\n"
  printf "Recommends: gnome-keyring, gnupg, libgl1, pinentry-gnome3, xdg-desktop-portal\n"
  printf "Section: mail\nPriority: optional\nHomepage: https://flectar.com\n"
  printf "Description: native, offline-first email client\n"
  printf " Yours is a native Rust and Slint desktop mail client with offline-first\n"
  printf " storage, Gmail and Microsoft account support, contacts, and calendars.\n"
  printf " .\n"
  printf " Mail sync, OAuth and the optional local Reacher address check use the\n"
  printf " network. Passwords, OAuth tokens and API keys are stored in the freedesktop\n"
  printf " Secret Service (GNOME Keyring, KWallet or KeePassXC), never in the message\n"
  printf " database. PDF preview needs a separately packaged libpdfium; without it PDF\n"
  printf " attachments open in the external viewer.\n"
} > "$root/DEBIAN/control"
chmod 644 "$root/DEBIAN/control"
( cd "$root" && find usr -type f -print0 | sort -z | xargs -0 md5sum > DEBIAN/md5sums )
chmod 644 "$root/DEBIAN/md5sums"
find "$root" -type d -exec chmod 755 {} +

mkdir -p "$out"
deb=$out/${pkg}_${version}_${arch}.deb
rm -f "$deb"
"$dpkgdeb" --build --root-owner-group "$root" "$deb"
echo "built $deb"
