use git2::{DiffOptions, Repository, Sort};
use gtk4::gdk::RGBA;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, DrawingArea, Label, ListBox, ListBoxRow,
    Orientation, Paned, ScrolledWindow, TextView,
};
use std::collections::HashMap;
use std::rc::Rc;

const APP_ID: &str = "com.github.gitkview";

// ── Commit data ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct CommitInfo {
    oid: git2::Oid,
    summary: String,
    author: String,
    time: i64,
    parents: Vec<git2::Oid>,
    refs: Vec<String>,
}

fn load_commits(repo: &Repository, max: usize) -> Vec<CommitInfo> {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    revwalk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL).ok();
    revwalk.push_head().ok();

    // Also push all branches
    if let Ok(branches) = repo.branches(None) {
        for branch in branches.flatten() {
            if let Some(oid) = branch.0.get().target() {
                revwalk.push(oid).ok();
            }
        }
    }

    let mut commits = Vec::new();
    let mut seen = std::collections::HashSet::new();

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

fn collect_refs(repo: &Repository, oid: git2::Oid) -> Vec<String> {
    let mut refs = Vec::new();
    if let Ok(references) = repo.references() {
        for reference in references.flatten() {
            if reference.target() == Some(oid) {
                if let Some(shorthand) = reference.shorthand() {
                    let name = reference.name().unwrap_or("");
                    if name.starts_with("refs/heads/")
                        || name.starts_with("refs/remotes/")
                        || name.starts_with("refs/tags/")
                    {
                        refs.push(shorthand.to_string());
                    }
                }
            }
        }
    }
    if let Ok(head) = repo.head() {
        if head.target() == Some(oid) && !refs.iter().any(|r| r == "HEAD") {
            refs.insert(0, "HEAD".to_string());
        }
    }
    refs
}

fn get_diff_text(repo: &Repository, oid: git2::Oid) -> String {
    let commit = match repo.find_commit(oid) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut opts = DiffOptions::new();
    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };

    let mut text = String::new();

    // Header
    text.push_str(&format!("commit {}\n", oid));
    text.push_str(&format!("Author: {}\n", commit.author()));
    let time = commit.time();
    if let Some(dt) = chrono::DateTime::from_timestamp(time.seconds(), 0) {
        text.push_str(&format!("Date:   {}\n", dt.format("%Y-%m-%d %H:%M:%S")));
    }
    text.push_str(&format!("\n    {}\n\n", commit.message().unwrap_or("")));

    // Diff stats
    if let Ok(stats) = diff.stats() {
        if let Ok(s) = stats.to_buf(git2::DiffStatsFormat::FULL, 80) {
            text.push_str(&format!("{}\n", s.as_str().unwrap_or("")));
        }
    }

    // Diff content
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let prefix = match line.origin() {
            '+' => "+",
            '-' => "-",
            ' ' => " ",
            _ => "",
        };
        text.push_str(prefix);
        text.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .ok();

    text
}

// ── Graph layout ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct GraphRow {
    node_col: usize,
    /// (from_col, to_col, color_col) — lines from this row to the next.
    /// from_col: where the line starts (in this row's column space)
    /// to_col: where the line ends (in the next row's column space)
    /// color_col: which column's color to use
    lines: Vec<(usize, usize, usize)>,
    num_cols: usize,
}

fn layout_graph(commits: &[CommitInfo]) -> Vec<GraphRow> {
    // `pipes` tracks what OID each lane is waiting for.
    let mut pipes: Vec<git2::Oid> = Vec::new();
    let mut rows = Vec::new();

    let oid_set: std::collections::HashSet<git2::Oid> = commits.iter().map(|c| c.oid).collect();

    for commit in commits {
        // Find which lane this commit lands in
        let node_col = pipes
            .iter()
            .position(|p| *p == commit.oid)
            .unwrap_or_else(|| {
                // Not expected by any lane — append a new one
                pipes.push(commit.oid);
                pipes.len() - 1
            });

        // Snapshot current pipe count for "this row"
        let num_cols_before = pipes.len();

        // Build the next row's pipes by replacing the current commit's lane
        // with its parents, and keeping all other lanes unchanged.
        let mut next_pipes: Vec<git2::Oid> = Vec::new();
        let mut lines: Vec<(usize, usize, usize)> = Vec::new();

        // Process each current lane
        for (col, &pipe_oid) in pipes.iter().enumerate() {
            if col == node_col {
                // This is the commit's lane — replace with parents
                continue; // handled below
            }
            // Other lanes continue straight
            let next_col = next_pipes.len();
            next_pipes.push(pipe_oid);
            lines.push((col, next_col, col));
        }

        // Now insert the commit's parents at the node's position
        let mut first_parent = true;
        for parent_oid in &commit.parents {
            if !oid_set.contains(parent_oid) {
                continue; // parent not in our commit list
            }
            // Check if this parent is already tracked in next_pipes
            if let Some(existing) = next_pipes.iter().position(|p| *p == *parent_oid) {
                // Merge line: node_col → existing lane
                lines.push((node_col, existing, node_col));
            } else {
                // New lane for this parent
                let target_col = if first_parent {
                    // Insert first parent at the node's position to keep the graph tight
                    let insert_pos = node_col.min(next_pipes.len());
                    next_pipes.insert(insert_pos, *parent_oid);
                    // Fix up all line targets that shifted
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

        // If no parents at all (root commit), the lane just ends
        // (no line from node_col)

        rows.push(GraphRow {
            node_col,
            lines,
            num_cols: num_cols_before.max(next_pipes.len()),
        });

        pipes = next_pipes;
    }

    rows
}

// ── Graph colors ─────────────────────────────────────────────────────────

const GRAPH_COLORS: &[&str] = &[
    "#cba6f7", "#94e2d5", "#f9e2af", "#a6e3a1", "#f5c2e7", "#89b4fa", "#fab387", "#89dceb",
];

fn color_for_col(col: usize) -> &'static str {
    GRAPH_COLORS[col % GRAPH_COLORS.len()]
}

// ── UI ───────────────────────────────────────────────────────────────────

fn build_ui(app: &Application) {
    let repo_path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());

    let repo = match Repository::discover(&repo_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Not a git repository: {e}");
            std::process::exit(1);
        }
    };

    let commits = load_commits(&repo, 2000);
    let graph_rows = layout_graph(&commits);
    let max_cols = graph_rows.iter().map(|r| r.num_cols).max().unwrap_or(1);

    let repo = Rc::new(repo);
    let commits = Rc::new(commits);
    let graph_rows = Rc::new(graph_rows);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("gitkview")
        .default_width(1200)
        .default_height(800)
        .build();

    let paned = Paned::new(Orientation::Vertical);
    paned.set_position(350);

    // ── Top: commit list with graph ──
    let top_scroll = ScrolledWindow::new();
    let list_box = ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    list_box.add_css_class("commit-list");

    let col_width = 12.0_f32;
    let row_height = 20.0_f32;
    // Cap visible graph lanes — beyond this, lanes are clipped
    let max_visible_lanes = 15;
    let graph_width = (max_visible_lanes as f32 * col_width + 10.0) as i32;

    for (idx, commit) in commits.iter().enumerate() {
        let row = ListBoxRow::new();
        let hbox = GtkBox::new(Orientation::Horizontal, 4);

        // Graph column
        let graph_area = DrawingArea::new();
        graph_area.set_content_width(graph_width);
        graph_area.set_content_height(row_height as i32);
        graph_area.set_overflow(gtk4::Overflow::Visible);

        let graph_rows_clone = graph_rows.clone();
        graph_area.set_draw_func(move |_area, cr, _w, h| {
            let gr = &graph_rows_clone[idx];
            let h = h as f64;
            let cw = col_width as f64;
            let node = gr.node_col;

            let set_col = |cr: &gtk4::cairo::Context, col: usize, alpha: f64| {
                let rgba = RGBA::parse(color_for_col(col)).unwrap_or(RGBA::new(0.8, 0.6, 1.0, 1.0));
                cr.set_source_rgba(
                    rgba.red() as f64,
                    rgba.green() as f64,
                    rgba.blue() as f64,
                    alpha,
                );
            };

            let x_of = |col: usize| -> f64 { col as f64 * cw + cw / 2.0 };
            let cy = h / 2.0;
            cr.set_line_width(2.0);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.set_line_join(gtk4::cairo::LineJoin::Round);

            // Each line is (from_col, to_col, color_col)
            for &(from, to, color) in &gr.lines {
                set_col(cr, color, if from == to { 0.5 } else { 0.7 });
                let x1 = x_of(from);
                let x2 = x_of(to);

                if from == to {
                    // Straight-through lane
                    if from == node {
                        // Node's own lane — draw top half and bottom half
                        // around the dot
                        cr.move_to(x1, -1.0);
                        cr.line_to(x1, cy - 4.0);
                        cr.stroke().ok();
                        cr.move_to(x1, cy + 4.0);
                        cr.line_to(x1, h + 1.0);
                        cr.stroke().ok();
                    } else {
                        cr.move_to(x1, -1.0);
                        cr.line_to(x2, h + 1.0);
                        cr.stroke().ok();
                    }
                } else {
                    // Branch/merge — smooth curve from node to target lane
                    cr.move_to(x1, cy);
                    cr.curve_to(x1, h * 0.85, x2, h * 0.85, x2, h + 1.0);
                    cr.stroke().ok();
                }
            }

            // Draw the incoming line to the dot (from top)
            if idx > 0 {
                let prev = &graph_rows_clone[idx - 1];
                let has_incoming = prev.lines.iter().any(|&(_, to, _)| to == node);
                if has_incoming {
                    set_col(cr, node, 0.7);
                    cr.move_to(x_of(node), -1.0);
                    cr.line_to(x_of(node), cy - 4.0);
                    cr.stroke().ok();
                }
            }

            // Draw commit dot
            set_col(cr, node, 1.0);
            cr.arc(x_of(node), cy, 3.5, 0.0, 2.0 * std::f64::consts::PI);
            cr.fill().ok();
        });

        hbox.append(&graph_area);

        // Ref labels
        for ref_name in &commit.refs {
            let ref_label = Label::new(Some(ref_name));
            ref_label.add_css_class("ref-label");
            if ref_name == "HEAD" {
                ref_label.add_css_class("ref-head");
            } else if ref_name.starts_with("origin/") {
                ref_label.add_css_class("ref-remote");
            } else {
                ref_label.add_css_class("ref-branch");
            }
            hbox.append(&ref_label);
        }

        // Summary
        let summary_label = Label::new(Some(&commit.summary));
        summary_label.set_xalign(0.0);
        summary_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary_label.set_hexpand(true);
        summary_label.add_css_class("commit-summary");
        hbox.append(&summary_label);

        // Author
        let author_label = Label::new(Some(&commit.author));
        author_label.add_css_class("commit-author");
        hbox.append(&author_label);

        // Date
        let date_str = chrono::DateTime::from_timestamp(commit.time, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let date_label = Label::new(Some(&date_str));
        date_label.add_css_class("commit-date");
        hbox.append(&date_label);

        row.set_child(Some(&hbox));
        list_box.append(&row);
    }

    top_scroll.set_child(Some(&list_box));

    // ── Bottom: diff view with syntax highlighting ──
    let diff_scroll = ScrolledWindow::new();
    let diff_view = TextView::new();
    diff_view.set_editable(false);
    diff_view.set_monospace(true);
    diff_view.set_wrap_mode(gtk4::WrapMode::None);
    diff_view.add_css_class("diff-view");
    diff_scroll.set_child(Some(&diff_view));

    let diff_buffer = diff_view.buffer();

    // Create text tags for diff highlighting
    let tag_table = diff_buffer.tag_table();

    let tag_add = gtk4::TextTag::builder()
        .name("add")
        .foreground("#a6e3a1")
        .build();
    let tag_del = gtk4::TextTag::builder()
        .name("del")
        .foreground("#f38ba8")
        .build();
    let tag_hunk = gtk4::TextTag::builder()
        .name("hunk")
        .foreground("#89b4fa")
        .build();
    let tag_meta = gtk4::TextTag::builder()
        .name("meta")
        .foreground("#cba6f7")
        .weight(700)
        .build();
    let tag_file = gtk4::TextTag::builder()
        .name("file")
        .foreground("#f9e2af")
        .weight(700)
        .build();
    let tag_stat = gtk4::TextTag::builder()
        .name("stat")
        .foreground("#6c7086")
        .build();

    tag_table.add(&tag_add);
    tag_table.add(&tag_del);
    tag_table.add(&tag_hunk);
    tag_table.add(&tag_meta);
    tag_table.add(&tag_file);
    tag_table.add(&tag_stat);

    // Selection handler — show diff for selected commit with highlighting
    let repo_clone = repo.clone();
    let commits_clone = commits.clone();
    list_box.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let idx = row.index() as usize;
            if let Some(commit) = commits_clone.get(idx) {
                let text = get_diff_text(&repo_clone, commit.oid);
                diff_buffer.set_text("");

                for line in text.lines() {
                    let end = diff_buffer.end_iter();
                    let offset = end.offset();
                    diff_buffer.insert(&mut diff_buffer.end_iter(), line);
                    diff_buffer.insert(&mut diff_buffer.end_iter(), "\n");

                    let start = diff_buffer.iter_at_offset(offset);
                    let end = diff_buffer.end_iter();

                    let tag_name = if line.starts_with('+') && !line.starts_with("+++") {
                        Some("add")
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        Some("del")
                    } else if line.starts_with("@@") {
                        Some("hunk")
                    } else if line.starts_with("diff ") || line.starts_with("index ") {
                        Some("meta")
                    } else if line.starts_with("--- ") || line.starts_with("+++ ") {
                        Some("file")
                    } else if line.starts_with("commit ")
                        || line.starts_with("Author:")
                        || line.starts_with("Date:")
                    {
                        Some("meta")
                    } else if line.contains(" | ")
                        && (line.contains(" +") || line.contains(" -") || line.ends_with("Bin"))
                    {
                        Some("stat")
                    } else {
                        None
                    };

                    if let Some(tag) = tag_name {
                        diff_buffer.apply_tag_by_name(tag, &start, &end);
                    }
                }
            }
        }
    });

    paned.set_start_child(Some(&top_scroll));
    paned.set_end_child(Some(&diff_scroll));
    window.set_child(Some(&paned));

    // ── CSS: Catppuccin Mocha ──
    let css = gtk4::CssProvider::new();
    css.load_from_data(
        r#"
        window {
            background-color: #1e1e2e;
            color: #cdd6f4;
        }
        .commit-list row {
            padding: 0px 8px;
            margin: 0;
            min-height: 20px;
            border-spacing: 0;
        }
        .commit-list {
            border-spacing: 0;
        }
        .commit-list row:selected {
            background-color: rgba(203, 166, 247, 0.2);
        }
        .commit-list row:hover {
            background-color: rgba(203, 166, 247, 0.08);
        }
        .commit-summary {
            color: #cdd6f4;
            font-family: monospace;
            font-size: 13px;
        }
        .commit-author {
            color: #94e2d5;
            font-family: monospace;
            font-size: 12px;
            margin-left: 12px;
            min-width: 120px;
        }
        .commit-date {
            color: #6c7086;
            font-family: monospace;
            font-size: 12px;
            margin-left: 8px;
            min-width: 130px;
        }
        .ref-label {
            font-family: monospace;
            font-size: 11px;
            font-weight: bold;
            padding: 1px 6px;
            margin: 0 2px;
            border-radius: 4px;
        }
        .ref-head {
            background-color: rgba(243, 139, 168, 0.3);
            color: #f38ba8;
        }
        .ref-branch {
            background-color: rgba(166, 227, 161, 0.2);
            color: #a6e3a1;
        }
        .ref-remote {
            background-color: rgba(137, 180, 250, 0.2);
            color: #89b4fa;
        }
        .diff-view {
            background-color: #1e1e2e;
            color: #cdd6f4;
            font-family: monospace;
            font-size: 13px;
        }
        textview text {
            background-color: #1e1e2e;
            color: #cdd6f4;
        }
    "#,
    );
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Select first commit
    if let Some(first_row) = list_box.row_at_index(0) {
        list_box.select_row(Some(&first_row));
    }

    window.present();
}

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}
