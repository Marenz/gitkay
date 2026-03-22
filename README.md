<h1 align="center">gitkview</h1>

<p align="center">
  <b>Native Wayland git history viewer</b><br>
  <i>A gitk replacement built with Rust + egui</i>
</p>

---

## Why

gitk is a Tcl/Tk app that needs XWayland, has stale X11 selection issues on Wayland, and looks dated. gitkview is a native Wayland app with a modern dark theme, built for browsing commit graphs and diffs.

## Features

- **Commit graph** — colored lanes with merge/branch visualization, consistent lane colors across column shifts
- **Diff view** — syntax-highlighted diffs (adds green, deletes red, hunks blue, file headers yellow, metadata purple)
- **File list sidebar** — clickable file names with +/- stats, click to jump to file's diff
- **Search** — filter commits by SHA, author name, commit message, or ref name
- **SHA copy** — click a commit to copy its SHA to clipboard
- **Ref labels** — colored badges for HEAD, branches, remotes, tags
- **Author colors** — each author gets a unique consistent color
- **Virtual scrolling** — handles repos with thousands of commits
- **Catppuccin Mocha** dark theme

## Install

### Requirements

- Rust 1.75+
- GTK4 dev libraries (for egui's Wayland backend): `gtk4-devel`, `libgraphene-devel`
- libgit2 dev: usually pulled in by `git2` crate, may need `openssl-devel`

### Build

```sh
git clone https://github.com/Marenz/gitkview
cd gitkview
cargo build --release
cp target/release/gitkview ~/.local/bin/
```

## Usage

```sh
# From inside a git repo
gitkview

# Or specify a path
gitkview /path/to/repo
```

### Controls

| Action | Effect |
|---|---|
| Click commit | Select, show diff, copy SHA |
| Scroll | Browse commit history |
| Type in search | Filter by SHA/author/message/ref |
| Click file in sidebar | Jump to that file's diff |
| Hover file | Show full path tooltip |

## Architecture

Single-file Rust app (`src/main.rs`):

- **git2** (libgit2) for repo access — revwalk, diff, refs
- **egui** + **eframe** for the UI — immediate-mode rendering on a Wayland-native window
- **chrono** for date formatting

The commit graph uses a lane-based layout algorithm:
- Pipes track active lanes with `Option<(Oid, color)>` slots
- First parent always continues in the node's column (no unnecessary shifts)
- Additional parents open new lanes in empty slots
- Convergence detected when multiple lanes point to the same commit
- Colors are tracked per-lane, not per-column, so they survive column shifts

## Screenshot

Launch on any git repo — the commit graph is on the left, commit messages in the center, author/date on the right, diff + file list at the bottom.

## License

MIT
