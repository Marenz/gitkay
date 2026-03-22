# gitkview

Native Wayland git history viewer — a gitk replacement built with Rust + egui.

## Build / Test

```sh
cargo build --release    # release build
cargo test               # graph layout tests (9 tests)
cp target/release/gitkview ~/.local/bin/
```

System dependencies (openSUSE): `gtk4-devel libgraphene-devel`

## Architecture

Single-file app at `src/main.rs`. Three main sections:

### Data Layer
- `load_commits()` — walks the git repo via `git2::Revwalk` (topological + time order), collects commit info, refs
- `collect_refs()` — maps OIDs to branch/tag/remote ref names
- `get_diff_data()` — generates diff lines with syntax classification + file list with per-file add/delete counts and diff line offsets

### Graph Layout (`layout_graph()`)
- **Pipes**: `Vec<Option<(Oid, color_index)>>` — each slot is a lane. `None` = empty slot.
- **Algorithm**: For each commit (newest first):
  1. Find which pipe slot matches this commit's OID (or create new slot)
  2. Check for convergence: multiple pipes pointing to the same commit → merge lines
  3. Clear the node's slot
  4. First parent reuses the node's column (same color). Even if parent is already tracked elsewhere, keep both lanes — convergence resolved when parent is processed.
  5. Additional parents get new lanes in empty slots or appended
  6. All other active pipes continue straight
  7. Trim trailing empty slots
- **Key invariant**: first parent always continues straight down in the same column → no unnecessary diagonals for linear history
- **Color tracking**: each pipe has a color index that persists through column shifts. `next_color` increments globally for new branches.

### UI (egui)
- **Top panel**: search bar (filters commits by SHA/author/message/ref)
- **Central panel**: commit graph + list with manual virtual scrolling (pre-spacer, painter, post-spacer)
- **Bottom panel**: horizontally split — left is diff view, right is file list sidebar
- **Graph rendering**: each edge `(from_col, to_col, color)` drawn as a line segment. Lines touching the node column split around the dot. First commits (no incoming line from above) skip the top half.
- **Text layout**: summary clipped to available width before author/date. Author colors via deterministic hash.

## Graph Tests

Tests use fake OIDs (`oid(n)`) and `CommitInfo` structs without a real git repo.

Key test cases:
- `test_linear_history` — 4 linear commits stay in col 0, no diagonals
- `test_simple_branch_and_merge` — merge commit has diagonal, first parent stays in col 0
- `test_linear_branch_no_diagonals` — two parallel branches, no false diagonals on linear commits
- `test_parallel_branches_stable_columns` — two independent branches keep their columns
- `test_pr_merge_pattern` — GitHub PR merge: main in col 0, PR branch in col 1
- `test_sequential_merges` — multiple PRs merged in sequence, main stays col 0
- `test_branch_after_merge_stays_stable` — convergence lines drawn correctly when branches meet

## Dependencies

- `eframe` / `egui` — native Wayland window + immediate-mode UI
- `git2` — libgit2 bindings for repo access
- `chrono` — date formatting

## Common Pitfalls

- egui's `show_rows` doesn't fill the viewport exactly → use manual virtualization with pre/post spacers
- `layout_no_wrap` + `with_clip_rect` for summary text truncation (egui `layout()` wraps text)
- Lane colors must be tracked per-pipe, not per-column-position, or they change when columns shift
- When two branches target the same parent, both keep their lanes going — convergence is handled when the parent commit is processed (finding multiple matching pipes)
