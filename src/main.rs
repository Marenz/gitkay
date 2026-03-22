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
    lines: Vec<(usize, usize, usize)>,
    num_cols: usize,
}

fn layout_graph(commits: &[CommitInfo]) -> Vec<GraphRow> {
    // Each pipe tracks (oid, color_index)
    let mut pipes: Vec<(git2::Oid, usize)> = Vec::new();
    let mut next_color: usize = 0;
    let mut rows = Vec::new();
    let oid_set: HashSet<git2::Oid> = commits.iter().map(|c| c.oid).collect();

    for commit in commits {
        let node_col = pipes
            .iter()
            .position(|p| p.0 == commit.oid)
            .unwrap_or_else(|| {
                let color = next_color;
                next_color += 1;
                pipes.push((commit.oid, color));
                pipes.len() - 1
            });

        let node_color = pipes[node_col].1;
        let num_cols_before = pipes.len();
        let mut next_pipes: Vec<(git2::Oid, usize)> = Vec::new();
        let mut lines: Vec<(usize, usize, usize)> = Vec::new();

        // Continue all other lanes (keep their color)
        for (col, &(pipe_oid, pipe_color)) in pipes.iter().enumerate() {
            if col == node_col {
                continue;
            }
            let next_col = next_pipes.len();
            next_pipes.push((pipe_oid, pipe_color));
            lines.push((col, next_col, pipe_color));
        }

        // Insert parents
        let mut first_parent = true;
        for parent_oid in &commit.parents {
            if !oid_set.contains(parent_oid) {
                continue;
            }
            if let Some(existing) = next_pipes.iter().position(|p| p.0 == *parent_oid) {
                // Merge into existing lane
                lines.push((node_col, existing, node_color));
            } else {
                // New lane for this parent
                let parent_color = if first_parent { node_color } else { next_color };
                if !first_parent {
                    next_color += 1;
                }
                let target_col = if first_parent {
                    let insert_pos = node_col.min(next_pipes.len());
                    next_pipes.insert(insert_pos, (*parent_oid, parent_color));
                    for line in &mut lines {
                        if line.1 >= insert_pos {
                            line.1 += 1;
                        }
                    }
                    insert_pos
                } else {
                    next_pipes.push((*parent_oid, parent_color));
                    next_pipes.len() - 1
                };
                lines.push((node_col, target_col, parent_color));
                first_parent = false;
            }
            if first_parent {
                first_parent = false;
            }
        }

        rows.push(GraphRow {
            node_col,
            lines,
            num_cols: num_cols_before.max(next_pipes.len()),
        });
        pipes = next_pipes;
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

                            if !touches_node {
                                // Straight or diagonal, doesn't touch the node
                                painter.line_segment(
                                    [egui::pos2(x_top, y_top), egui::pos2(x_bot, y_bottom)],
                                    stroke,
                                );
                            } else if from == to && from == gr.node_col {
                                // Node's own lane continuation: split around dot
                                painter.line_segment(
                                    [
                                        egui::pos2(x_top, y_top),
                                        egui::pos2(x_top, y_center - dot_radius - 1.0),
                                    ],
                                    stroke,
                                );
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
                            graph_color(gr.node_col),
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
