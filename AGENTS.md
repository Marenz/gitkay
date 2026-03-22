# gitkay

Native Wayland git history viewer — gitk, but okay. Built with Rust + egui.

## Build / Test

```sh
cargo build --release
cargo test                # 14 graph layout tests
cp target/release/gitkay ~/.local/bin/
```

System dependencies (openSUSE): `gtk4-devel libgraphene-devel openssl-devel`

## Architecture

Single-file app at `src/main.rs` (~1600 lines). Three sections:

### Data Layer
- `load_commits()` — revwalk via `git2`, topological + time order, precomputed ref map
- `build_ref_map()` — single pass over all refs, O(refs) instead of O(commits × refs)
- `get_diff_data()` — diff lines with syntax classification + file list with per-file stats and line offsets

### Graph Layout (`layout_graph()`)
- **Pipes**: `Vec<Option<(Oid, color_index)>>` — fixed column slots, `None` = empty
- **Algorithm** per commit:
  1. Find matching pipe(s). Multiple matches = convergence → merge lines + clear extras
  2. Clear node slot. First parent reuses node column (same color). Even if parent tracked elsewhere, keep both — convergence resolves at parent's row
  3. Additional parents get new lanes in empty slots (tracked as `new_lanes`)
  4. Other active pipes continue straight. Skip `new_lanes` (no vertical stub)
  5. Add convergence lines. Trim trailing empty slots
- **Key invariant**: first parent always continues straight → no false diagonals
- **Color tracking**: per-pipe color index, persists through column shifts

### UI (egui immediate mode)
- **Top panel**: search bar (SHA/author/message/ref), Enter cycles matches
- **Central panel**: commit graph + list. Manual virtual scrolling (pre-spacer, painter, post-spacer). Lazy loading (200 initial, +500 on scroll-near-bottom)
- **Bottom panel**: diff view (left, syntax-highlighted) + file list sidebar (right, dynamic width)
- **Graph rendering**: each edge `(from, to, color)` = one line segment. Lines touching node split around dot. No incoming line for first commits (no parent above)
- **Text**: summary clipped via `with_clip_rect`. Authors colored by hash. Refs colored by name hash (12-color extended palette)
- **Clipboard**: SHA copied to both clipboard + primary selection on click

## Tests (14)

All use fake OIDs via `oid(n)` — no real git repo needed.

- `test_linear_history` — 4 commits, col 0, no diagonals
- `test_simple_branch_and_merge` — merge diagonal, first parent col 0
- `test_linear_branch_no_diagonals` — parallel branches, no false diagonals
- `test_parallel_branches_stable_columns` — independent branches keep columns
- `test_pr_merge_pattern` — GitHub PR: main col 0, PR col 1
- `test_sequential_merges` — multiple PRs, main stays col 0
- `test_branch_after_merge_stays_stable` — convergence drawn correctly
- `test_merge_commit_has_diagonal` — merge has at least one diagonal
- `test_merge_new_lane_no_vertical` — new merge lane: no vertical stub
- `test_merge_new_lane_no_vertical_but_diagonal` — diagonal present, no vertical
- `test_merge_new_lane_no_vertical_even_with_pending_commit` — even with future commit
- `test_merge_into_feature_main_continues` — main vertical persists after merge-into
- `test_convergence_no_vertical_on_consumed_lane` — consumed lanes: no vertical
- `test_many_linear_commits_stay_in_column` — 10 linear commits, all col 0

## Common Pitfalls

- egui `show_rows` leaves a gap → manual virtualization with pre/post spacers
- `layout_no_wrap` + `with_clip_rect` for text truncation (egui `layout()` wraps)
- Lane colors: track per-pipe, not per-column, or colors change on shifts
- Two branches → same parent: both keep lanes, convergence at parent row
- New merge lanes: skip vertical (diagonal already connects, no source above)
- `collect_refs` per commit is O(commits × refs) → precompute ref map once
