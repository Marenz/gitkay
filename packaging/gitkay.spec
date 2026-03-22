Name:           gitkay
Version:        1.0.0
Release:        1%{?dist}
Summary:        A fast, native Wayland git history viewer
License:        MIT
URL:            https://github.com/Marenz/gitkay
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.75
BuildRequires:  cargo
BuildRequires:  gtk4-devel
BuildRequires:  libgraphene-devel
BuildRequires:  openssl-devel
BuildRequires:  pkg-config
BuildRequires:  cmake

%description
gitkay is a native Wayland git history viewer — gitk, but okay.
Features a commit graph with colored branch lanes, syntax-highlighted
diffs, file list sidebar, search, and Catppuccin Mocha dark theme.
Built with Rust + egui for fast startup and smooth scrolling.

%prep
%autosetup

%build
cargo build --release

%install
install -Dm755 target/release/gitkay %{buildroot}%{_bindir}/gitkay

%files
%license LICENSE
%doc README.md
%{_bindir}/gitkay

%changelog
* Sat Mar 22 2026 Marenz <marenz@supradigital.org> - 1.0.0-1
- Initial release
- Commit graph with colored lanes and merge visualization
- Syntax-highlighted diff viewer with file list sidebar
- Search by SHA, author, message, branch, tag
- Native Wayland, Catppuccin Mocha theme
