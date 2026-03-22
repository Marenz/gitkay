use eframe::egui;
use git2::{DiffOptions, Repository, Sort};
use std::collections::{HashMap, HashSet};

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

fn get_diff_text(repo: &Repository, oid: git2::Oid) -> Vec<DiffLine> {
    let commit = match repo.find_commit(oid) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut opts = DiffOptions::new();
    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut lines = Vec::new();

    // Header
    lines.push(DiffLine::new(&format!("commit {}", oid), LineKind::Meta));
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

    // Patch
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let kind = match line.origin() {
            '+' => LineKind::Add,
            '-' => LineKind::Del,
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
        let text = format!("{prefix}{}", content.trim_end_matches('\n'));
        lines.push(DiffLine::new(&text, kind));
        true
    })
    .ok();

    lines
}

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

// ── Graph layout ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct GraphRow {
    node_col: usize,
    /// (from_col, to_col, color_col)
    lines: Vec<(usize, usize, usize)>,
    num_cols: usize,
}

fn layout_graph(commits: &[CommitInfo]) -> Vec<GraphRow> {
    let mut pipes: Vec<git2::Oid> = Vec::new();
    let mut rows = Vec::new();
    let oid_set: HashSet<git2::Oid> = commits.iter().map(|c| c.oid).collect();

    for commit in commits {
        let node_col = pipes
            .iter()
            .position(|p| *p == commit.oid)
            .unwrap_or_else(|| {
                pipes.push(commit.oid);
                pipes.len() - 1
            });

        let num_cols_before = pipes.len();
        let mut next_pipes: Vec<git2::Oid> = Vec::new();
        let mut lines: Vec<(usize, usize, usize)> = Vec::new();

        // Continue all other lanes
        for (col, &pipe_oid) in pipes.iter().enumerate() {
            if col == node_col {
                continue;
            }
            let next_col = next_pipes.len();
            next_pipes.push(pipe_oid);
            lines.push((col, next_col, col));
        }

        // Insert parents
        let mut first_parent = true;
        for parent_oid in &commit.parents {
            if !oid_set.contains(parent_oid) {
                continue;
            }
            if let Some(existing) = next_pipes.iter().position(|p| *p == *parent_oid) {
                lines.push((node_col, existing, node_col));
            } else {
                let target_col = if first_parent {
                    let insert_pos = node_col.min(next_pipes.len());
                    next_pipes.insert(insert_pos, *parent_oid);
                    for line in &mut lines {
                        if line.1 >= insert_pos {
                            line.1 += 1;
                        }
                    }
                    insert_pos
                } else {
                    next_pipes.push(*parent_oid);
                    next_pipes.len() - 1
                };
                lines.push((
                    node_col,
                    target_col,
                    if first_parent { node_col } else { target_col },
                ));
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

// Catppuccin Mocha
const BG: egui::Color32 = egui::Color32::from_rgb(30, 30, 46);
const TEXT: egui::Color32 = egui::Color32::from_rgb(205, 214, 244);
const SUBTEXT: egui::Color32 = egui::Color32::from_rgb(108, 112, 134);
const SURFACE0: egui::Color32 = egui::Color32::from_rgb(49, 50, 68);
const MAUVE: egui::Color32 = egui::Color32::from_rgb(203, 166, 247);
const GREEN: egui::Color32 = egui::Color32::from_rgb(166, 227, 161);
const RED: egui::Color32 = egui::Color32::from_rgb(243, 139, 168);
const BLUE: egui::Color32 = egui::Color32::from_rgb(137, 180, 250);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(249, 226, 175);
const TEAL: egui::Color32 = egui::Color32::from_rgb(148, 226, 213);

// ── App state ────────────────────────────────────────────────────────────

struct GitkApp {
    commits: Vec<CommitInfo>,
    graph_rows: Vec<GraphRow>,
    selected: Option<usize>,
    diff_lines: Vec<DiffLine>,
    repo_path: String,
}

impl GitkApp {
    fn new(cc: &eframe::CreationContext<'_>, repo_path: String) -> Self {
        // Set up dark theme
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
            repo_path,
        }
    }
}

impl eframe::App for GitkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let row_height = 20.0;
        let col_width = 12.0;
        let dot_radius = 3.5;
        let max_graph_cols = 20;

        // Split: top = commit list, bottom = diff
        egui::TopBottomPanel::bottom("diff_panel")
            .resizable(true)
            .min_height(100.0)
            .default_height(300.0)
            .show(ctx, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(13.0));
                egui::ScrollArea::both().show(ui, |ui| {
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
            });

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

            egui::ScrollArea::vertical().show_rows(ui, row_height, num_commits, |ui, row_range| {
                // Get the paint area for the graph
                let top_left = ui.cursor().min;

                // Draw all visible rows
                let painter = ui.painter().clone();

                for idx in row_range.clone() {
                    let commit = &self.commits[idx];
                    let gr = &self.graph_rows[idx];
                    let y_center =
                        top_left.y + (idx - row_range.start) as f32 * row_height + row_height / 2.0;
                    let y_top = y_center - row_height / 2.0;
                    let y_bottom = y_center + row_height / 2.0;

                    // Row background (selection / hover)
                    let row_rect = egui::Rect::from_min_size(
                        egui::pos2(top_left.x, y_top),
                        egui::vec2(ui.available_width(), row_height),
                    );

                    let is_selected = self.selected == Some(idx);
                    if is_selected {
                        painter.rect_filled(
                            row_rect,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(203, 166, 247, 30),
                        );
                    }

                    // Click detection
                    let response = ui.allocate_rect(row_rect, egui::Sense::click());
                    if response.clicked() {
                        self.selected = Some(idx);
                        let repo = Repository::discover(&self.repo_path).unwrap();
                        self.diff_lines = get_diff_text(&repo, commit.oid);
                    }
                    if response.hovered() && !is_selected {
                        painter.rect_filled(
                            row_rect,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(203, 166, 247, 12),
                        );
                    }

                    let gx = |col: usize| -> f32 {
                        top_left.x + col as f32 * col_width + col_width / 2.0
                    };

                    // ── Graph: straight-through lanes ──
                    for &(from, to, color_col) in &gr.lines {
                        if from == to && from != gr.node_col {
                            let x = gx(from);
                            let c = graph_color(color_col).linear_multiply(0.5);
                            painter.line_segment(
                                [egui::pos2(x, y_top), egui::pos2(x, y_bottom)],
                                egui::Stroke::new(2.0, c),
                            );
                        }
                    }

                    // ── Graph: incoming line (top → dot) ──
                    if idx > 0 {
                        let prev = &self.graph_rows[idx - 1];
                        if prev.lines.iter().any(|&(_, to, _)| to == gr.node_col) {
                            let x = gx(gr.node_col);
                            let c = graph_color(gr.node_col).linear_multiply(0.7);
                            painter.line_segment(
                                [egui::pos2(x, y_top), egui::pos2(x, y_center - dot_radius)],
                                egui::Stroke::new(2.0, c),
                            );
                        }
                    }

                    // ── Graph: outgoing line (dot → bottom) ──
                    if gr
                        .lines
                        .iter()
                        .any(|&(f, t, _)| f == gr.node_col && t == gr.node_col)
                    {
                        let x = gx(gr.node_col);
                        let c = graph_color(gr.node_col).linear_multiply(0.7);
                        painter.line_segment(
                            [
                                egui::pos2(x, y_center + dot_radius),
                                egui::pos2(x, y_bottom),
                            ],
                            egui::Stroke::new(2.0, c),
                        );
                    }

                    // ── Graph: branch/merge diagonals ──
                    for &(from, to, color_col) in &gr.lines {
                        if from != to {
                            let c = graph_color(color_col).linear_multiply(0.7);
                            painter.line_segment(
                                [egui::pos2(gx(from), y_center), egui::pos2(gx(to), y_bottom)],
                                egui::Stroke::new(2.0, c),
                            );
                        }
                    }

                    // ── Graph: commit dot ──
                    let dot_color = graph_color(gr.node_col);
                    painter.circle_filled(
                        egui::pos2(gx(gr.node_col), y_center),
                        dot_radius,
                        dot_color,
                    );

                    // ── Text: refs, summary, author, date ──
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

                    // Summary
                    let summary_font = egui::FontId::monospace(13.0);
                    let summary_galley =
                        painter.layout_no_wrap(commit.summary.clone(), summary_font, TEXT);
                    painter.galley(
                        egui::pos2(cursor_x + 4.0, y_center - 7.0),
                        summary_galley,
                        TEXT,
                    );

                    // Author (right-aligned area)
                    let right_x = row_rect.max.x;
                    let date_str = chrono::DateTime::from_timestamp(commit.time, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    let date_font = egui::FontId::monospace(12.0);
                    let date_galley = painter.layout_no_wrap(date_str, date_font.clone(), SUBTEXT);
                    let date_w = date_galley.size().x;
                    painter.galley(
                        egui::pos2(right_x - date_w - 8.0, y_center - 7.0),
                        date_galley,
                        SUBTEXT,
                    );

                    let author_galley =
                        painter.layout_no_wrap(commit.author.clone(), date_font, TEAL);
                    let author_w = author_galley.size().x;
                    painter.galley(
                        egui::pos2(right_x - date_w - author_w - 20.0, y_center - 7.0),
                        author_galley,
                        TEAL,
                    );
                }
            });
        });
    }
}

fn main() -> eframe::Result {
    let repo_path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());

    // Verify repo exists before launching UI
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
