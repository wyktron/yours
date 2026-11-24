# Native RPM for Yours, built from the repository tarball.
#
#   git archive --prefix=yours-0.1.0-alpha.7/ -o ~/rpmbuild/SOURCES/yours-0.1.0-alpha.7.tar.gz HEAD
#   rpmbuild -ba packaging/rpm/yours.spec
#
# Cargo is asked for crates.io at build time: this repository does not ship a
# vendor tarball. For an offline build run `cargo vendor` and add a source
# replacement to .cargo/config.toml, then build with CARGO_NET_OFFLINE=true.
#
# The date and version below follow Cargo.toml. Keep them in step when the
# workspace version changes.
%global upstream_version 0.1.0-alpha.7
%global debug_package %{nil}
%{!?_licensedir:%global _licensedir %{_datadir}/licenses}

Name:           yours
Version:        0.1.0~alpha.7
Release:        1%{?dist}
Summary:        Native, offline-first email client
License:        AGPL-3.0-only
URL:            https://flectar.com
Source0:        %{name}-%{upstream_version}.tar.gz
ExclusiveArch:  x86_64 aarch64

# The Rust workspace and the C libraries Slint, winit and the document
# renderer look for while linking. The executable itself links only libdbus-1
# beyond libc; the display, font and XCB libraries are dlopen'ed, which is why
# the runtime Requires below are explicit.
BuildRequires:  cargo
BuildRequires:  rust >= 1.95
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
BuildRequires:  dbus-devel
BuildRequires:  fontconfig-devel
BuildRequires:  libX11-devel
BuildRequires:  libXcursor-devel
BuildRequires:  libxcb-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  libxkbcommon-x11-devel
BuildRequires:  wayland-devel
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

Requires:       dbus-libs
Requires:       fontconfig
Requires:       freetype
Requires:       hicolor-icon-theme
Requires:       libX11
Requires:       libX11-xcb
Requires:       libXcursor
Requires:       libXi
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       xdg-utils
Recommends:     gnome-keyring
Recommends:     gnupg2
Recommends:     pinentry
Recommends:     xdg-desktop-portal

%description
Yours is a native Rust and Slint mail client with offline-first storage,
Gmail and Microsoft account support, contacts, and calendars.

Mail sync, OAuth, and the local Reacher address check use the network.
Credentials are kept in the freedesktop Secret Service, never in the message
database. PDF preview loads libpdfium.so from %{_libdir}/flectar-mail when a
distribution package provides it; otherwise PDF attachments open externally.

%prep
%autosetup -n %{name}-%{upstream_version}

%build
# Default features are remote-content plus gpu-renderer, the same set the
# release builds use. The flatpak feature is deliberately not enabled here:
# native packages keep the system tray and direct desktop notifications.
%set_build_flags
cargo build --release --locked --bin yours

%install
install -Dpm0755 target/release/yours %{buildroot}%{_bindir}/yours
install -Dpm0644 resources/org.yours.mail.desktop \
    %{buildroot}%{_datadir}/applications/org.yours.mail.desktop
install -Dpm0644 resources/org.yours.mail.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/org.yours.mail.metainfo.xml
install -Dpm0644 resources/app-icon/icon-seal-512.png \
    %{buildroot}%{_datadir}/icons/hicolor/512x512/apps/org.yours.mail.png
install -Dpm0644 resources/app-icon/icon-seal.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/org.yours.mail.svg

mkdir -p %{buildroot}%{_licensedir}/%{name}/LICENSES
install -pm0644 LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
cp -a LICENSES/. %{buildroot}%{_licensedir}/%{name}/LICENSES/
install -Dpm0644 resources/fonts/google-sans-flex/OFL.txt \
    %{buildroot}%{_licensedir}/%{name}/google-sans-flex/OFL.txt
install -Dpm0644 THIRD_PARTY_NOTICES.md \
    %{buildroot}%{_docdir}/%{name}/THIRD_PARTY_NOTICES.md
install -Dpm0644 resources/fonts/google-sans-flex/README.md \
    %{buildroot}%{_docdir}/%{name}/google-sans-flex/README.md

%check
# Same desktop and AppStream checks the release pipeline runs.
desktop-file-validate resources/org.yours.mail.desktop
appstreamcli validate --no-net resources/org.yours.mail.metainfo.xml

%files
%{_bindir}/yours
%{_datadir}/applications/org.yours.mail.desktop
%{_datadir}/metainfo/org.yours.mail.metainfo.xml
%{_datadir}/icons/hicolor/512x512/apps/org.yours.mail.png
%{_datadir}/icons/hicolor/scalable/apps/org.yours.mail.svg
%license %{_licensedir}/%{name}/LICENSE
%license %{_licensedir}/%{name}/LICENSES/*
%license %{_licensedir}/%{name}/google-sans-flex/OFL.txt
%doc %{_docdir}/%{name}

%changelog
* Wed Nov 25 2026 Flectar <flectar@users.noreply.github.com> - 0.1.0~alpha.7-1
- Package the Yours preview for RPM-based distributions
