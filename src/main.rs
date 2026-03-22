use eframe::egui;
use git2::{DiffOptions, Repository, Sort};
use std::collections::HashSet;

// ── Commit data ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct CommitInfo {
    oid: git2::Oid,
    summary: String,
    author: String,
    time: i64,
    parents: Vec<git2::Oid>,
    refs: Vec<(String, RefKind)>,
}

#[derive(Clone, PartialEq)]
enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}

fn load_commits(repo: &Repository, max: usize) -> Vec<CommitInfo> {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    revwalk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL).ok();
    revwalk.push_head().ok();
    if let Ok(branches) = repo.branches(None) {
        for branch in branches.flatten() {
            if let Some(oid) = branch.0.get().target() {
                revwalk.push(oid).ok();
            }
        }
    }
    let mut commits = Vec::new();
    let mut seen = HashSet::new();
    for oid in revwalk.flatten() {
        if !seen.insert(oid) {
            continue;
        }
        if let Ok(commit) = repo.find_commit(oid) {
            let refs = collect_refs(repo, oid);
            commits.push(CommitInfo {
                oid,
                summary: commit.summary().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
                time: commit.time().seconds(),
                parents: commit.parent_ids().collect(),
                refs,
            });
            if commits.len() >= max {
                break;
            }
        }
    }
    commits
}

fn collect_refs(repo: &Repository, oid: git2::Oid) -> Vec<(String, RefKind)> {
    let mut refs = Vec::new();
    if let Ok(references) = repo.references() {
        for reference in references.flatten() {
            if reference.target() == Some(oid) {
                if let Some(shorthand) = reference.shorthand() {
                    let name = reference.name().unwrap_or("");
                    let kind = if name.starts_with("refs/tags/") {
                        RefKind::Tag
                    } else if name.starts_with("refs/remotes/") {
                        RefKind::Remote
                    } else if name.starts_with("refs/heads/") {
                        RefKind::Branch
                    } else {
                        continue;
                    };
                    refs.push((shorthand.to_string(), kind));
                }
            }
        }
    }
    if let Ok(head) = repo.head() {
        if head.target() == Some(oid) {
            refs.insert(0, ("HEAD".to_string(), RefKind::Head));
        }
    }
    refs
}

// ── Diff data ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct DiffLine {
    text: String,
    kind: LineKind,
}

impl DiffLine {
    fn new(text: &str, kind: LineKind) -> Self {
        Self {
            text: text.to_string(),
            kind,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Context,
    Add,
    Del,
    Hunk,
    Meta,
    FileMeta,
    FileName,
    Stat,
}

#[derive(Clone)]
struct FileEntry {
    path: String,
    additions: usize,
    deletions: usize,
    diff_line_idx: usize, // line index in diff_lines where this file's diff starts
}

struct DiffData {
    lines: Vec<DiffLine>,
    files: Vec<FileEntry>,
}

fn get_diff_data(repo: &Repository, oid: git2::Oid) -> DiffData {
    let commit = match repo.find_commit(oid) {
        Ok(c) => c,
        Err(_) => {
            return DiffData {
                lines: Vec::new(),
                files: Vec::new(),
            }
        }
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => {
            return DiffData {
                lines: Vec::new(),
                files: Vec::new(),
            }
        }
    };
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut opts = DiffOptions::new();
    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => {
            return DiffData {
                lines: Vec::new(),
                files: Vec::new(),
            }
        }
    };

    // Collect file stats
    let mut files = Vec::new();
    for i in 0..diff.deltas().len() {
        if let Some(delta) = diff.get_delta(i) {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            files.push(FileEntry {
                path,
                additions: 0,
                deletions: 0,
                diff_line_idx: 0,
            });
        }
    }

    let mut lines = Vec::new();

    // Header
    lines.push(DiffLine::new(&format!("commit {oid}"), LineKind::Meta));
    lines.push(DiffLine::new(
        &format!("Author: {}", commit.author()),
        LineKind::Meta,
    ));
    if let Some(dt) = chrono::DateTime::from_timestamp(commit.time().seconds(), 0) {
        lines.push(DiffLine::new(
            &format!("Date:   {}", dt.format("%Y-%m-%d %H:%M:%S")),
            LineKind::Meta,
        ));
    }
    lines.push(DiffLine::new("", LineKind::Context));
    if let Some(msg) = commit.message() {
        for l in msg.lines() {
            lines.push(DiffLine::new(&format!("    {l}"), LineKind::Meta));
        }
    }
    lines.push(DiffLine::new("", LineKind::Context));

    // Stats
    if let Ok(stats) = diff.stats() {
        if let Ok(s) = stats.to_buf(git2::DiffStatsFormat::FULL, 80) {
            for l in s.as_str().unwrap_or("").lines() {
                lines.push(DiffLine::new(l, LineKind::Stat));
            }
        }
    }
    lines.push(DiffLine::new("", LineKind::Context));

    // Patch — track which file we're in
    let mut current_file_idx: Option<usize> = None;
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        // Detect file boundary
        let delta_path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or("");

        if current_file_idx.is_none()
            || files
                .get(current_file_idx.unwrap())
                .is_none_or(|f| f.path != delta_path)
        {
            current_file_idx = files.iter().position(|f| f.path == delta_path);
            if let Some(fi) = current_file_idx {
                files[fi].diff_line_idx = lines.len();
            }
        }

        let kind = match line.origin() {
            '+' => {
                if let Some(fi) = current_file_idx {
                    files[fi].additions += 1;
                }
                LineKind::Add
            }
            '-' => {
                if let Some(fi) = current_file_idx {
                    files[fi].deletions += 1;
                }
                LineKind::Del
            }
            'H' | 'F' => LineKind::Hunk,
            _ => {
                let content = std::str::from_utf8(line.content()).unwrap_or("");
                if content.starts_with("diff ") || content.starts_with("index ") {
                    LineKind::FileMeta
                } else if content.starts_with("--- ") || content.starts_with("+++ ") {
                    LineKind::FileName
                } else if content.starts_with("@@") {
                    LineKind::Hunk
                } else {
                    LineKind::Context
                }
            }
        };
        let prefix = match line.origin() {
            '+' => "+",
            '-' => "-",
            _ => "",
        };
        let content = std::str::from_utf8(line.content()).unwrap_or("");
        lines.push(DiffLine::new(
            &format!("{prefix}{}", content.trim_end_matches('\n')),
            kind,
        ));
        true
    })
    .ok();

    DiffData { lines, files }
}

// ── Graph layout ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct GraphRow {
    node_col: usize,
    node_color: usize,
    lines: Vec<(usize, usize, usize)>,
    num_cols: usize,
}

fn layout_graph(commits: &[CommitInfo]) -> Vec<GraphRow> {
    // Each pipe tracks (oid, color_index). None = empty slot.
    let mut pipes: Vec<Option<(git2::Oid, usize)>> = Vec::new();
    let mut next_color: usize = 0;
    let mut rows = Vec::new();
    let oid_set: HashSet<git2::Oid> = commits.iter().map(|c| c.oid).collect();

    for commit in commits {
        // Find which column this commit is in
        // Find which column this commit is in. If multiple lanes point
        // to this commit (convergence), pick the first and mark others
        // for merge lines.
        let matching_cols: Vec<usize> = pipes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_some_and(|(oid, _)| oid == commit.oid))
            .map(|(i, _)| i)
            .collect();

        let node_col = if matching_cols.is_empty() {
            // New commit — find an empty slot or append
            let color = next_color;
            next_color += 1;
            if let Some(pos) = pipes.iter().position(|p| p.is_none()) {
                pipes[pos] = Some((commit.oid, color));
                pos
            } else {
                pipes.push(Some((commit.oid, color)));
                pipes.len() - 1
            }
        } else {
            matching_cols[0]
        };

        let node_color = pipes[node_col].unwrap().1;

        // Extra lanes that also pointed to this commit — they converge here.
        let mut converge_lines: Vec<(usize, usize, usize)> = Vec::new();
        if matching_cols.len() > 1 {
            for &col in &matching_cols[1..] {
                let color = pipes[col].unwrap().1;
                converge_lines.push((col, node_col, color));
                pipes[col] = None;
            }
        }

        let mut lines: Vec<(usize, usize, usize)> = Vec::new();
        let mut new_lanes: Vec<usize> = Vec::new(); // columns created by this commit

        // Clear the node's slot
        pipes[node_col] = None;

        // First parent takes the node's slot (same column, same color).
        // If the first parent is already tracked in another lane (convergence),
        // still continue in the node's column — the other lane will merge at
        // the parent's own row.
        let mut first_parent = true;
        for parent_oid in &commit.parents {
            if !oid_set.contains(parent_oid) {
                continue;
            }

            // Check if parent is already tracked in a different lane
            let existing = pipes
                .iter()
                .position(|p| p.is_some_and(|(oid, _)| oid == *parent_oid));

            if first_parent {
                if let Some(existing_col) = existing {
                    if existing_col == node_col {
                        // Parent is in our own column (shouldn't happen but handle it)
                        lines.push((node_col, node_col, node_color));
                    } else {
                        // Parent is tracked in another lane. Keep our lane going
                        // toward the parent — the duplicate will be resolved when
                        // the parent commit is processed (it will find two lanes
                        // pointing to it and pick one).
                        pipes[node_col] = Some((*parent_oid, node_color));
                        lines.push((node_col, node_col, node_color));
                    }
                } else {
                    // Parent not tracked yet — claim the node's column
                    pipes[node_col] = Some((*parent_oid, node_color));
                    lines.push((node_col, node_col, node_color));
                }
            } else {
                // Second+ parent
                if let Some(existing_col) = existing {
                    // Merge: draw line from node to existing lane
                    lines.push((node_col, existing_col, node_color));
                } else {
                    // Additional parent: find empty slot or append
                    let color = next_color;
                    next_color += 1;
                    let col = if let Some(pos) = pipes.iter().position(|p| p.is_none()) {
                        pipes[pos] = Some((*parent_oid, color));
                        pos
                    } else {
                        pipes.push(Some((*parent_oid, color)));
                        pipes.len() - 1
                    };
                    lines.push((node_col, col, color));
                    new_lanes.push(col);
                }
            }
            first_parent = false;
        }

        // All other active lanes continue straight — but skip:
        // - lanes consumed by convergence (pipe already cleared)
        // - lanes newly created by this commit's merge (nothing above them)
        for (col, pipe) in pipes.iter().enumerate() {
            if col == node_col {
                continue;
            }
            if new_lanes.contains(&col) {
                continue;
            }
            if let Some((_, color)) = pipe {
                lines.push((col, col, *color));
            }
        }

        // Add convergence lines (other lanes that pointed to this commit)
        lines.extend(converge_lines);

        let num_cols = pipes.len();
        rows.push(GraphRow {
            node_col,
            node_color,
            lines,
            num_cols,
        });

        // Trim trailing empty slots
        while pipes.last() == Some(&None) {
            pipes.pop();
        }
    }
    rows
}

// ── Colors ───────────────────────────────────────────────────────────────

const GRAPH_COLORS: &[(u8, u8, u8)] = &[
    (203, 166, 247), // mauve
    (148, 226, 213), // teal
    (249, 226, 175), // yellow
    (166, 227, 161), // green
    (245, 194, 231), // pink
    (137, 180, 250), // blue
    (250, 179, 135), // peach
    (137, 220, 235), // sky
];

fn graph_color(col: usize) -> egui::Color32 {
    let (r, g, b) = GRAPH_COLORS[col % GRAPH_COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

/// Deterministic color for an author name.
fn author_color(name: &str) -> egui::Color32 {
    let hash = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = GRAPH_COLORS[(hash as usize) % GRAPH_COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

const BG: egui::Color32 = egui::Color32::from_rgb(30, 30, 46);
const TEXT: egui::Color32 = egui::Color32::from_rgb(205, 214, 244);
const SUBTEXT: egui::Color32 = egui::Color32::from_rgb(108, 112, 134);
const SURFACE0: egui::Color32 = egui::Color32::from_rgb(49, 50, 68);
const MAUVE: egui::Color32 = egui::Color32::from_rgb(203, 166, 247);
const GREEN: egui::Color32 = egui::Color32::from_rgb(166, 227, 161);
const RED: egui::Color32 = egui::Color32::from_rgb(243, 139, 168);
const BLUE: egui::Color32 = egui::Color32::from_rgb(137, 180, 250);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(249, 226, 175);

// ── App state ────────────────────────────────────────────────────────────

struct GitkApp {
    commits: Vec<CommitInfo>,
    graph_rows: Vec<GraphRow>,
    selected: Option<usize>,
    diff_lines: Vec<DiffLine>,
    diff_files: Vec<FileEntry>,
    diff_scroll_to: Option<usize>,
    repo_path: String,
    search_text: String,
    search_matches: Vec<usize>, // indices into commits
    copied_toast: Option<std::time::Instant>,
}

impl GitkApp {
    fn new(cc: &eframe::CreationContext<'_>, repo_path: String) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = BG;
        style.visuals.extreme_bg_color = BG;
        style.visuals.faint_bg_color = SURFACE0;
        style.visuals.override_text_color = Some(TEXT);
        cc.egui_ctx.set_style(style);

        let repo = Repository::discover(&repo_path).expect("Not a git repository");
        let commits = load_commits(&repo, 5000);
        let graph_rows = layout_graph(&commits);

        Self {
            commits,
            graph_rows,
            selected: None,
            diff_lines: Vec::new(),
            diff_files: Vec::new(),
            diff_scroll_to: None,
            repo_path,
            search_text: String::new(),
            search_matches: Vec::new(),
            copied_toast: None,
        }
    }
}

impl eframe::App for GitkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let row_height = 20.0;
        let col_width = 12.0;
        let dot_radius = 3.5;
        let max_graph_cols = 20;

        // ── Top panel: search bar ──
        egui::TopBottomPanel::top("search_panel")
            .exact_height(28.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new("🔍").size(14.0));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .desired_width(300.0)
                            .hint_text("Search SHA, author, message...")
                            .font(egui::FontId::monospace(13.0)),
                    );
                    if resp.changed() {
                        let q = self.search_text.to_lowercase();
                        if q.is_empty() {
                            self.search_matches.clear();
                        } else {
                            self.search_matches = self
                                .commits
                                .iter()
                                .enumerate()
                                .filter(|(_, c)| {
                                    c.summary.to_lowercase().contains(&q)
                                        || c.author.to_lowercase().contains(&q)
                                        || c.oid.to_string().starts_with(&q)
                                        || c.refs.iter().any(|(r, _)| r.to_lowercase().contains(&q))
                                })
                                .map(|(i, _)| i)
                                .collect();
                        }
                    }
                    if !self.search_matches.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("{} matches", self.search_matches.len()))
                                .color(SUBTEXT)
                                .size(12.0),
                        );
                    }
                    // Copied toast
                    if let Some(t) = self.copied_toast {
                        if t.elapsed().as_secs_f32() < 2.0 {
                            ui.label(egui::RichText::new("SHA copied!").color(GREEN).size(12.0));
                        } else {
                            self.copied_toast = None;
                        }
                    }
                });
            });

        // ── Bottom panel: diff view + file list ──
        egui::TopBottomPanel::bottom("diff_panel")
            .resizable(true)
            .min_height(100.0)
            .default_height(300.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(4, 0)),
            )
            .show(ctx, |ui| {
                // Wider resize grab area
                ui.add_space(3.0);
                ui.separator();
                ui.add_space(2.0);

                ui.horizontal_top(|ui| {
                    // Sidebar width driven by file names + stats
                    let sidebar_width = if self.diff_files.is_empty() {
                        0.0
                    } else {
                        let max_entry_len = self
                            .diff_files
                            .iter()
                            .map(|f| {
                                let name_len = f.path.rsplit('/').next().unwrap_or(&f.path).len();
                                let stat_len = format!("+{} -{}", f.additions, f.deletions).len();
                                name_len + stat_len + 2
                            })
                            .max()
                            .unwrap_or(10);
                        ((max_entry_len as f32 * 7.5) + 30.0).clamp(140.0, 280.0)
                    };

                    // Left: diff content (scrollable)
                    let diff_width = ui.available_width() - sidebar_width - 10.0;
                    ui.allocate_ui_with_layout(
                        egui::vec2(diff_width.max(200.0), ui.available_height()),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.style_mut().override_font_id = Some(egui::FontId::monospace(13.0));
                            let mut scroll = egui::ScrollArea::both().id_salt("diff_scroll");

                            if let Some(target_line) = self.diff_scroll_to.take() {
                                let target_y = target_line as f32 * 16.0;
                                scroll = scroll.vertical_scroll_offset(target_y);
                            }

                            scroll.show(ui, |ui| {
                                for line in &self.diff_lines {
                                    let color = match line.kind {
                                        LineKind::Add => GREEN,
                                        LineKind::Del => RED,
                                        LineKind::Hunk => BLUE,
                                        LineKind::Meta => MAUVE,
                                        LineKind::FileMeta => MAUVE,
                                        LineKind::FileName => YELLOW,
                                        LineKind::Stat => SUBTEXT,
                                        LineKind::Context => TEXT,
                                    };
                                    ui.colored_label(color, &line.text);
                                }
                            });
                        },
                    );

                    ui.separator();

                    // Right: file list with hover effects
                    ui.allocate_ui_with_layout(
                        egui::vec2(sidebar_width, ui.available_height()),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            if !self.diff_files.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("{} files", self.diff_files.len()))
                                        .color(SUBTEXT)
                                        .size(11.0),
                                );
                                ui.add_space(4.0);
                            }
                            egui::ScrollArea::vertical()
                                .id_salt("file_list")
                                .show(ui, |ui| {
                                    for file in &self.diff_files {
                                        let short_path =
                                            file.path.rsplit('/').next().unwrap_or(&file.path);
                                        let line_idx = file.diff_line_idx;

                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 18.0),
                                            egui::Sense::click(),
                                        );

                                        // Hover highlight
                                        if resp.hovered() {
                                            ui.painter().rect_filled(
                                                rect,
                                                2.0,
                                                egui::Color32::from_rgba_unmultiplied(
                                                    203, 166, 247, 20,
                                                ),
                                            );
                                        }

                                        let mut x = rect.min.x + 4.0;
                                        let cy = rect.center().y;

                                        // File name first
                                        let name_color = if resp.hovered() {
                                            egui::Color32::from_rgb(220, 224, 252)
                                        } else {
                                            TEXT
                                        };
                                        let ng = ui.painter().layout_no_wrap(
                                            short_path.to_string(),
                                            egui::FontId::monospace(12.0),
                                            name_color,
                                        );
                                        ui.painter().galley(
                                            egui::pos2(x, cy - 7.0),
                                            ng.clone(),
                                            name_color,
                                        );
                                        x += ng.size().x + 6.0;

                                        // Then stats
                                        if file.additions > 0 {
                                            let g = ui.painter().layout_no_wrap(
                                                format!("+{}", file.additions),
                                                egui::FontId::monospace(10.0),
                                                GREEN,
                                            );
                                            ui.painter().galley(
                                                egui::pos2(x, cy - 6.0),
                                                g.clone(),
                                                GREEN,
                                            );
                                            x += g.size().x + 3.0;
                                        }
                                        if file.deletions > 0 {
                                            let g = ui.painter().layout_no_wrap(
                                                format!("-{}", file.deletions),
                                                egui::FontId::monospace(10.0),
                                                RED,
                                            );
                                            ui.painter().galley(
                                                egui::pos2(x, cy - 6.0),
                                                g.clone(),
                                                RED,
                                            );
                                        }

                                        if resp.clicked() {
                                            self.diff_scroll_to = Some(line_idx);
                                        }
                                        if resp.hovered() {
                                            resp.show_tooltip_text(&file.path);
                                        }
                                    }
                                });
                        },
                    );
                });
            });

        // ── Central panel: commit graph + list ──
        egui::CentralPanel::default().show(ctx, |ui| {
            let num_commits = self.commits.len();
            let graph_width = (self
                .graph_rows
                .iter()
                .map(|r| r.num_cols)
                .max()
                .unwrap_or(1)
                .min(max_graph_cols) as f32)
                * col_width
                + 8.0;

            let panel_height = ui.available_height();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Total content height
                    let total_content = num_commits as f32 * row_height;
                    let total_height = total_content.max(panel_height);

                    // Spacer before visible rows
                    let scroll_offset = ui.clip_rect().min.y - ui.cursor().min.y;
                    let first_row = (scroll_offset / row_height).floor().max(0.0) as usize;
                    let visible_rows = (panel_height / row_height).ceil() as usize + 2;
                    let last_row = (first_row + visible_rows).min(num_commits);
                    let row_range = first_row..last_row;

                    // Pre-spacer
                    if first_row > 0 {
                        ui.allocate_space(egui::vec2(0.0, first_row as f32 * row_height));
                    }

                    let rows_height = (last_row - first_row) as f32 * row_height;
                    let (response, painter) = ui.allocate_painter(
                        egui::vec2(ui.available_width(), rows_height),
                        egui::Sense::click(),
                    );
                    let top_left = response.rect.min;

                    // Check click — select commit and copy SHA
                    if response.clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let row_offset = ((pos.y - top_left.y) / row_height) as usize;
                            let clicked_idx = row_range.start + row_offset;
                            if clicked_idx < num_commits {
                                self.selected = Some(clicked_idx);
                                let commit = &self.commits[clicked_idx];
                                // Copy SHA to clipboard
                                ctx.copy_text(commit.oid.to_string());
                                self.copied_toast = Some(std::time::Instant::now());
                                let repo = Repository::discover(&self.repo_path).unwrap();
                                let data = get_diff_data(&repo, commit.oid);
                                self.diff_lines = data.lines;
                                self.diff_files = data.files;
                            }
                        }
                    }

                    for idx in row_range.clone() {
                        let commit = &self.commits[idx];
                        let gr = &self.graph_rows[idx];
                        let row_offset = (idx - row_range.start) as f32;
                        let y_center = top_left.y + row_offset * row_height + row_height / 2.0;
                        let y_top = y_center - row_height / 2.0;
                        let y_bottom = y_center + row_height / 2.0;

                        // Row background
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(top_left.x, y_top),
                            egui::vec2(response.rect.width(), row_height),
                        );

                        let is_search_match = !self.search_matches.is_empty()
                            && self.search_matches.binary_search(&idx).is_ok();

                        if self.selected == Some(idx) {
                            painter.rect_filled(
                                row_rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(203, 166, 247, 30),
                            );
                        } else if is_search_match {
                            painter.rect_filled(
                                row_rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(249, 226, 175, 18),
                            );
                        } else if response.hover_pos().is_some_and(|p| row_rect.contains(p)) {
                            painter.rect_filled(
                                row_rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(203, 166, 247, 12),
                            );
                        }

                        let gx = |col: usize| -> f32 {
                            top_left.x + col as f32 * col_width + col_width / 2.0
                        };

                        // ── Graph ──
                        // Each edge (from, to, color) represents a line in this
                        // row from column `from` at y_top to column `to` at y_bottom.
                        for &(from, to, color_col) in &gr.lines {
                            let c = graph_color(color_col).linear_multiply(if from == to {
                                0.5
                            } else {
                                0.7
                            });
                            let stroke = egui::Stroke::new(2.0, c);
                            let x_top = gx(from);
                            let x_bot = gx(to);

                            // Check if this line passes through the node
                            let touches_node = from == gr.node_col || to == gr.node_col;

                            // Check if this node has an incoming line from above
                            let has_incoming = idx > 0
                                && self.graph_rows[idx - 1]
                                    .lines
                                    .iter()
                                    .any(|&(_, to, _)| to == gr.node_col);

                            if !touches_node {
                                // Straight or diagonal, doesn't touch the node
                                painter.line_segment(
                                    [egui::pos2(x_top, y_top), egui::pos2(x_bot, y_bottom)],
                                    stroke,
                                );
                            } else if from == to && from == gr.node_col {
                                // Node's own lane continuation: split around dot
                                if has_incoming {
                                    painter.line_segment(
                                        [
                                            egui::pos2(x_top, y_top),
                                            egui::pos2(x_top, y_center - dot_radius - 1.0),
                                        ],
                                        stroke,
                                    );
                                }
                                painter.line_segment(
                                    [
                                        egui::pos2(x_bot, y_center + dot_radius + 1.0),
                                        egui::pos2(x_bot, y_bottom),
                                    ],
                                    stroke,
                                );
                            } else if from == gr.node_col {
                                // Outgoing from node: dot center → target column bottom
                                painter.line_segment(
                                    [
                                        egui::pos2(gx(gr.node_col), y_center),
                                        egui::pos2(x_bot, y_bottom),
                                    ],
                                    stroke,
                                );
                            } else if to == gr.node_col {
                                // Incoming to node: source column top → dot center
                                painter.line_segment(
                                    [
                                        egui::pos2(x_top, y_top),
                                        egui::pos2(gx(gr.node_col), y_center),
                                    ],
                                    stroke,
                                );
                            }
                        }

                        // Commit dot
                        painter.circle_filled(
                            egui::pos2(gx(gr.node_col), y_center),
                            dot_radius,
                            graph_color(gr.node_color),
                        );

                        // ── Text ──
                        let text_x = top_left.x + graph_width;
                        let mut cursor_x = text_x;

                        // Ref labels
                        for (ref_name, kind) in &commit.refs {
                            let (bg, fg) = match kind {
                                RefKind::Head => (
                                    egui::Color32::from_rgba_unmultiplied(243, 139, 168, 80),
                                    RED,
                                ),
                                RefKind::Branch => (
                                    egui::Color32::from_rgba_unmultiplied(166, 227, 161, 50),
                                    GREEN,
                                ),
                                RefKind::Remote => (
                                    egui::Color32::from_rgba_unmultiplied(137, 180, 250, 50),
                                    BLUE,
                                ),
                                RefKind::Tag => (
                                    egui::Color32::from_rgba_unmultiplied(249, 226, 175, 50),
                                    YELLOW,
                                ),
                            };
                            let font = egui::FontId::monospace(11.0);
                            let galley = painter.layout_no_wrap(ref_name.clone(), font, fg);
                            let label_w = galley.size().x + 10.0;
                            let label_rect = egui::Rect::from_min_size(
                                egui::pos2(cursor_x, y_center - 8.0),
                                egui::vec2(label_w, 16.0),
                            );
                            painter.rect_filled(label_rect, 4.0, bg);
                            painter.galley(egui::pos2(cursor_x + 5.0, y_center - 7.0), galley, fg);
                            cursor_x += label_w + 4.0;
                        }

                        // Author + date (right-aligned) — compute first to know where summary must stop
                        let right_x = row_rect.max.x;
                        let date_str = chrono::DateTime::from_timestamp(commit.time, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        let date_font = egui::FontId::monospace(12.0);
                        let date_galley =
                            painter.layout_no_wrap(date_str, date_font.clone(), SUBTEXT);
                        let date_w = date_galley.size().x;

                        let a_color = author_color(&commit.author);
                        let author_galley =
                            painter.layout_no_wrap(commit.author.clone(), date_font, a_color);
                        let author_w = author_galley.size().x;

                        let author_date_x = right_x - date_w - author_w - 28.0;

                        // Summary — truncate to available space before author
                        let summary_max_w = (author_date_x - cursor_x - 12.0).max(20.0);
                        let summary_font = egui::FontId::monospace(13.0);
                        let summary_galley =
                            painter.layout_no_wrap(commit.summary.clone(), summary_font, TEXT);
                        // Clip to not overflow into author/date
                        let summary_clip = egui::Rect::from_min_max(
                            egui::pos2(cursor_x + 4.0, y_top),
                            egui::pos2(cursor_x + 4.0 + summary_max_w, y_bottom),
                        );
                        painter.with_clip_rect(summary_clip).galley(
                            egui::pos2(cursor_x + 4.0, y_center - 7.0),
                            summary_galley,
                            TEXT,
                        );

                        // Draw author + date
                        painter.galley(
                            egui::pos2(author_date_x, y_center - 7.0),
                            author_galley,
                            a_color,
                        );
                        painter.galley(
                            egui::pos2(right_x - date_w - 8.0, y_center - 7.0),
                            date_galley,
                            SUBTEXT,
                        );
                    }

                    // Post-spacer to maintain correct total scroll height
                    let drawn_bottom = last_row as f32 * row_height;
                    let remaining = total_height - drawn_bottom;
                    if remaining > 0.0 {
                        ui.allocate_space(egui::vec2(0.0, remaining));
                    }
                });
        });
    }
}

fn main() -> eframe::Result {
    let repo_path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());

    if Repository::discover(&repo_path).is_err() {
        eprintln!("Not a git repository: {repo_path}");
        std::process::exit(1);
    }

    let title = {
        let repo = Repository::discover(&repo_path).unwrap();
        let workdir = repo
            .workdir()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("gitkview");
        format!("gitkview — {workdir}")
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title(&title),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| Ok(Box::new(GitkApp::new(cc, repo_path)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a fake OID from an integer for testing.
    fn oid(n: u32) -> git2::Oid {
        let mut bytes = [0u8; 20];
        bytes[..4].copy_from_slice(&n.to_be_bytes());
        git2::Oid::from_bytes(&bytes).unwrap()
    }

    /// Build a CommitInfo for testing. Commits are listed in topological
    /// order (newest first), just like `load_commits` returns.
    fn commit(id: u32, parents: &[u32]) -> CommitInfo {
        CommitInfo {
            oid: oid(id),
            summary: format!("Commit {id}"),
            author: "test".into(),
            time: 0,
            parents: parents.iter().map(|p| oid(*p)).collect(),
            refs: vec![],
        }
    }

    /// Assert that a specific commit's node stays in the same column as
    /// its first parent in the next row (linear continuation).
    fn assert_linear(rows: &[GraphRow], commits: &[CommitInfo], child: u32, parent: u32) {
        let child_idx = commits.iter().position(|c| c.oid == oid(child)).unwrap();
        let parent_idx = commits.iter().position(|c| c.oid == oid(parent)).unwrap();
        let child_col = rows[child_idx].node_col;
        let parent_col = rows[parent_idx].node_col;
        assert_eq!(
            child_col, parent_col,
            "Linear commit {child} (col {child_col}) should be in same column as parent {parent} (col {parent_col})"
        );
    }

    /// Assert a commit is in a specific column.
    fn assert_col(rows: &[GraphRow], commits: &[CommitInfo], id: u32, expected_col: usize) {
        let idx = commits.iter().position(|c| c.oid == oid(id)).unwrap();
        assert_eq!(
            rows[idx].node_col, expected_col,
            "Commit {id} should be in column {expected_col}, got {}",
            rows[idx].node_col
        );
    }

    /// Assert no diagonal lines exist for a commit (all edges are straight).
    fn assert_no_diagonals(rows: &[GraphRow], commits: &[CommitInfo], id: u32) {
        let idx = commits.iter().position(|c| c.oid == oid(id)).unwrap();
        for &(from, to, _) in &rows[idx].lines {
            assert_eq!(
                from, to,
                "Commit {id} has unexpected diagonal: col {from} → col {to}"
            );
        }
    }

    /// Assert that a lane's color is consistent: if a lane continues from
    /// row A to row B in a given column, the color should be the same.
    fn assert_colors_consistent(rows: &[GraphRow]) {
        for i in 1..rows.len() {
            let prev = &rows[i - 1];
            let curr = &rows[i];
            // For each straight-through lane in curr, find the matching
            // lane in prev that targets the same column
            for &(from, to, color) in &curr.lines {
                if from == to {
                    // Find the prev row edge that targets this column
                    for &(pf, pt, pc) in &prev.lines {
                        if pt == from && pf == pt {
                            // Same column straight-through in both rows
                            assert_eq!(
                                pc, color,
                                "Color inconsistency at row {i}: column {from} has color {color} but previous row had {pc}"
                            );
                        }
                    }
                }
            }
        }
    }

    // ── Test cases ──

    #[test]
    fn test_linear_history() {
        // A → B → C → D (simple linear)
        let commits = vec![
            commit(1, &[2]),
            commit(2, &[3]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        assert_col(&rows, &commits, 1, 0);
        assert_linear(&rows, &commits, 1, 2);
        assert_linear(&rows, &commits, 2, 3);
        assert_linear(&rows, &commits, 3, 4);
        assert_no_diagonals(&rows, &commits, 1);
        assert_no_diagonals(&rows, &commits, 2);
        assert_no_diagonals(&rows, &commits, 3);
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_simple_branch_and_merge() {
        //   1 (merge: parents 2, 3)
        //  / \
        // 2   3
        //  \ /
        //   4
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        // Commit 1 starts in column 0
        assert_col(&rows, &commits, 1, 0);
        // First parent (2) should stay in column 0
        assert_linear(&rows, &commits, 1, 2);
        // Commit 3 should be in a different column
        assert_ne!(
            rows[2].node_col, rows[1].node_col,
            "Branch commit 3 should be in different column from 2"
        );
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_linear_branch_no_diagonals() {
        // main:   1 → 2 → 5
        // branch: 3 → 4 (branched from 2, not yet merged)
        // Topological order: 1, 3, 2, 4, 5
        // Wait — topological + time order means children before parents.
        // Actually: 3 is newer than 2 but 1 is newest.
        // 1's parent is 2, 3's parent is 2, 2's parent is 5, 4 is...
        // Let me simplify:
        //
        // Commits in order (newest first):
        // 1 (parent: 2)  — latest on main
        // 3 (parent: 4)  — latest on branch
        // 2 (parent: 5)  — main continues
        // 4 (parent: 5)  — branch continues
        // 5 (parent: none) — root
        let commits = vec![
            commit(1, &[2]),
            commit(3, &[4]),
            commit(2, &[5]),
            commit(4, &[5]),
            commit(5, &[]),
        ];
        let rows = layout_graph(&commits);

        // 1 and 2 should be in the same column (linear on main)
        assert_linear(&rows, &commits, 1, 2);
        // 3 and 4 should be in the same column (linear on branch)
        assert_linear(&rows, &commits, 3, 4);
        // No diagonals for linear commits
        assert_no_diagonals(&rows, &commits, 2);
        assert_no_diagonals(&rows, &commits, 4);
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_merge_commit_has_diagonal() {
        // 1 (merge: parents 2 and 3)
        // 2 (parent: 4)
        // 3 (parent: 4)
        // 4 (root)
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        // Commit 1 should have at least one diagonal (the merge)
        let has_diagonal = rows[0].lines.iter().any(|&(f, t, _)| f != t);
        assert!(has_diagonal, "Merge commit 1 should have a diagonal edge");
    }

    #[test]
    fn test_many_linear_commits_stay_in_column() {
        // 10 linear commits: 1→2→3→...→10
        let commits: Vec<_> = (1..=10)
            .map(|i| {
                if i == 10 {
                    commit(i, &[])
                } else {
                    commit(i, &[i + 1])
                }
            })
            .collect();
        let rows = layout_graph(&commits);

        for i in 0..9 {
            assert_linear(&rows, &commits, i as u32 + 1, i as u32 + 2);
            assert_no_diagonals(&rows, &commits, i as u32 + 1);
        }
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_parallel_branches_stable_columns() {
        // Two parallel branches that don't interact:
        // Branch A: 1→3→5
        // Branch B: 2→4→6
        // Interleaved by time: 1, 2, 3, 4, 5, 6
        let commits = vec![
            commit(1, &[3]),
            commit(2, &[4]),
            commit(3, &[5]),
            commit(4, &[6]),
            commit(5, &[]),
            commit(6, &[]),
        ];
        let rows = layout_graph(&commits);

        // Branch A stays in one column
        assert_linear(&rows, &commits, 1, 3);
        assert_linear(&rows, &commits, 3, 5);
        // Branch B stays in another column
        assert_linear(&rows, &commits, 2, 4);
        assert_linear(&rows, &commits, 4, 6);
        // They should be in different columns
        assert_ne!(rows[0].node_col, rows[1].node_col);
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_branch_after_merge_stays_stable() {
        // 1 (merge: 2, 3)
        // 2 (parent: 4)
        // 3 (parent: 4)
        // 4 (parent: 5)
        // 5 (root)
        // Commit 4 will have a convergence diagonal (lane from 3 merges in)
        // but commit 4 itself should be in col 0 (main line)
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[5]),
            commit(5, &[]),
        ];
        let rows = layout_graph(&commits);

        assert_linear(&rows, &commits, 4, 5);
        // Commit 4 has a convergence line (branch lane merging in) — that's correct
        let has_convergence = rows[3].lines.iter().any(|&(f, t, _)| f != t);
        assert!(
            has_convergence,
            "Commit 4 should have convergence line from branch"
        );
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_pr_merge_pattern() {
        // Typical GitHub PR merge pattern:
        // 1 = merge commit (parents: 2, 3)
        // 2 = previous main commit (parent: 5)
        // 3 = PR head commit (parent: 4)
        // 4 = PR commit (parent: 5)
        // 5 = older main commit (root)
        //
        // Expected: main line (1→2→5) in col 0, PR branch (3→4) in col 1
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[5]),
            commit(3, &[4]),
            commit(4, &[5]),
            commit(5, &[]),
        ];
        let rows = layout_graph(&commits);

        // Main line stays in column 0
        assert_col(&rows, &commits, 1, 0);
        assert_linear(&rows, &commits, 1, 2);
        // PR commits should be linear with each other
        assert_linear(&rows, &commits, 3, 4);
        // After merge resolves, commit 5 should be in main column
        assert_linear(&rows, &commits, 2, 5);
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_merge_new_lane_no_vertical_but_diagonal() {
        // A merge commit's newly created lane should have the diagonal
        // but NO vertical in the merge row. The renderer draws the
        // incoming line for the next row from the diagonal's endpoint.
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        let merge_row = &rows[0];
        let has_diagonal = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == merge_row.node_col && t != f);
        assert!(has_diagonal, "Merge commit should have a diagonal edge");

        let target_col = merge_row
            .lines
            .iter()
            .find(|&&(f, t, _)| f == merge_row.node_col && t != f)
            .unwrap()
            .1;
        let has_vertical = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == target_col && t == target_col);
        assert!(
            !has_vertical,
            "Newly created merge lane (col {target_col}) should not have vertical"
        );
    }

    #[test]
    fn test_merge_into_feature_main_continues() {
        // Main is merged INTO a feature branch. Main's lane is newly
        // created by the merge, so NO vertical in the merge row. But
        // in subsequent rows (before commit 3 appears), main's lane
        // should have verticals.
        //
        // 1 (merge: 2, 3)  — feature merges main in
        // 2 (parent: 4)    — feature branch continues
        // 3 (parent: 5)    — main continues
        // 4 (parent: 6)    — feature
        // 5 (parent: 6)    — main
        // 6 (root)
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[5]),
            commit(4, &[6]),
            commit(5, &[6]),
            commit(6, &[]),
        ];
        let rows = layout_graph(&commits);

        let merge_row = &rows[0];
        let main_col = rows[2].node_col; // commit 3's column

        // Merge row has diagonal to main
        let has_diagonal = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == merge_row.node_col && t == main_col);
        assert!(has_diagonal, "Merge should have diagonal to main's column");

        // Merge row should NOT have vertical for new lane
        let has_vertical_at_merge = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == main_col && t == main_col);
        assert!(
            !has_vertical_at_merge,
            "New merge lane should not have vertical in merge row"
        );

        // But row 1 (commit 2) SHOULD have main's vertical continuation
        let row_2 = &rows[1]; // commit 2
        let has_main_vertical = row_2
            .lines
            .iter()
            .any(|&(f, t, _)| f == main_col && t == main_col);
        assert!(
            has_main_vertical,
            "Main lane (col {main_col}) must continue vertically in rows after the merge"
        );

        // Main should be linear: 3 → 5
        assert_linear(&rows, &commits, 3, 5);
        assert_colors_consistent(&rows);
    }

    #[test]
    fn test_merge_new_lane_no_vertical_even_with_pending_commit() {
        // Even though the newly created lane has a pending commit,
        // the merge row should NOT have a vertical for it. The
        // renderer handles the incoming line for the next row.
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        let merge_row = &rows[0];
        let target_col = merge_row
            .lines
            .iter()
            .find(|&&(f, t, _)| f == merge_row.node_col && t != f)
            .unwrap()
            .1;

        let has_vertical = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == target_col && t == target_col);
        assert!(
            !has_vertical,
            "New merge lane should not have vertical in merge row"
        );
    }

    #[test]
    fn test_convergence_no_vertical_on_consumed_lane() {
        // When two lanes converge at a commit, the consumed lane should
        // NOT have a vertical continuation.
        // 1 (merge: 2, 3)
        // 2 (parent: 4)    — both 2 and 3 point to 4
        // 3 (parent: 4)
        // 4 (parent: 5)
        // 5 (root)
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[5]),
            commit(5, &[]),
        ];
        let rows = layout_graph(&commits);

        // At commit 4 (row 3): two lanes converge. The consumed lane
        // should not have a vertical continuation.
        let conv_row = &rows[3]; // commit 4
        let convergence_sources: Vec<usize> = conv_row
            .lines
            .iter()
            .filter(|&&(f, t, _)| f != t && t == conv_row.node_col)
            .map(|&(f, _, _)| f)
            .collect();

        for src_col in &convergence_sources {
            let has_vertical = conv_row
                .lines
                .iter()
                .any(|&(f, t, _)| f == *src_col && t == *src_col);
            assert!(
                !has_vertical,
                "Consumed convergence lane (col {src_col}) should not have vertical"
            );
        }
    }

    #[test]
    fn test_merge_new_lane_no_vertical() {
        // A merge commit creates a NEW lane for its second parent.
        // That new lane should NOT have a vertical continuation in the
        // merge row because nothing is feeding it from above — only
        // the merge diagonal connects to it.
        //
        // 1 (merge: 2, 3)
        // 2 (parent: 4)
        // 3 (parent: 4)
        // 4 (root)
        //
        // At row 0: commit 1 creates a diagonal to col 1 for commit 3.
        // Col 1 should NOT also have a vertical (1,1) — there's nothing
        // above it, so the vertical is a stub hanging in empty space.
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4]),
            commit(3, &[4]),
            commit(4, &[]),
        ];
        let rows = layout_graph(&commits);

        let merge_row = &rows[0];
        // Find the new lane created by the merge
        let new_lane_col = merge_row
            .lines
            .iter()
            .find(|&&(f, t, _)| f == merge_row.node_col && t != f)
            .unwrap()
            .1;

        // This lane was JUST created by the merge — nothing above it.
        // It should NOT have a vertical continuation.
        let has_vertical = merge_row
            .lines
            .iter()
            .any(|&(f, t, _)| f == new_lane_col && t == new_lane_col);
        assert!(
            !has_vertical,
            "Newly created merge lane (col {new_lane_col}) should not have vertical — nothing feeds it from above"
        );
    }

    #[test]
    fn test_sequential_merges() {
        // Multiple PRs merged in sequence:
        // 1 (merge: 2, 3)  — merge PR-A
        // 2 (merge: 4, 5)  — merge PR-B
        // 3 (parent: 4)    — PR-A commit
        // 4 (parent: 6)    — main
        // 5 (parent: 6)    — PR-B commit
        // 6 (root)
        let commits = vec![
            commit(1, &[2, 3]),
            commit(2, &[4, 5]),
            commit(3, &[4]),
            commit(4, &[6]),
            commit(5, &[6]),
            commit(6, &[]),
        ];
        let rows = layout_graph(&commits);

        // Main line: 1→2→4→6 should all be in col 0
        assert_col(&rows, &commits, 1, 0);
        assert_linear(&rows, &commits, 1, 2);
        assert_linear(&rows, &commits, 2, 4);
        assert_linear(&rows, &commits, 4, 6);
        assert_colors_consistent(&rows);
    }
}
