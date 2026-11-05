use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget, Wrap},
};

use crate::{
    app::AppContext,
    config::DetailFileView,
    git::{Commit, FileChange, Ref},
};

#[derive(Debug, Default)]
pub struct CommitDetailState {
    height: usize,
    offset: usize,
    pub selected_file: usize,
    pub hovered_action: Option<usize>,
    pub hover_file: Option<usize>,
    last_selected_file: usize,
    avatar_row: Option<u16>,
    pending_avatar_delete_row: Option<u16>,
    /// Screen row of the first scrolled line and the line where the files
    /// start, recorded at render to map mouse rows to files.
    content_top: u16,
    changes_start: usize,
}

impl CommitDetailState {
    pub fn drain_pending_avatar_delete(&mut self) -> Option<u16> {
        self.pending_avatar_delete_row.take()
    }

    pub fn content_line_at(&self, row: u16) -> Option<usize> {
        let below = row.checked_sub(self.content_top)?;
        (below < self.height as u16).then(|| self.offset + below as usize)
    }

    pub fn changes_start(&self) -> usize {
        self.changes_start
    }
}

impl CommitDetailState {
    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
    }

    pub fn scroll_page_down(&mut self) {
        self.offset = self.offset.saturating_add(self.height);
    }

    pub fn scroll_page_up(&mut self) {
        self.offset = self.offset.saturating_sub(self.height);
    }

    pub fn scroll_half_page_down(&mut self) {
        self.offset = self.offset.saturating_add(self.height / 2);
    }

    pub fn scroll_half_page_up(&mut self) {
        self.offset = self.offset.saturating_sub(self.height / 2);
    }

    pub fn select_first(&mut self) {
        self.offset = 0;
        self.selected_file = 0;
    }

    pub fn select_last(&mut self) {
        self.offset = usize::MAX;
    }

    pub fn select_last_file(&mut self, total: usize) {
        self.selected_file = total.saturating_sub(1);
    }

    pub fn select_next_file(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        if self.selected_file < total.saturating_sub(1) {
            self.selected_file += 1;
        }
    }

    pub fn select_prev_file(&mut self) {
        if self.selected_file > 0 {
            self.selected_file -= 1;
        }
    }

    fn ensure_selected_visible(&mut self, selected_line: usize) {
        if self.selected_file != self.last_selected_file {
            self.last_selected_file = self.selected_file;
            if selected_line < self.offset {
                self.offset = selected_line;
            } else if selected_line >= self.offset + self.height {
                self.offset = selected_line.saturating_sub(self.height - 1);
            }
        }
    }
}

pub struct CommitDetail<'a> {
    commit: &'a Commit,
    changes: &'a Vec<FileChange>,
    refs: &'a Vec<Ref>,
    ctx: Rc<AppContext>,
    head_branch_name: Option<String>,
    is_head_commit: bool,
    /// `Some(n)` when this commit was opened from a PR drilldown
    /// surfaces as `· PR #n` next to the panel title so the user
    /// always sees the originating context.
    pr_origin: Option<u64>,
    /// Off when the diff pane takes the git-actions column's place.
    show_actions: bool,
    focus: PanelFocus,
}

/// Focused part of a detail panel; `None` when the commit list has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    #[default]
    None,
    Files,
    Diff,
}

impl<'a> CommitDetail<'a> {
    pub fn new(
        commit: &'a Commit,
        changes: &'a Vec<FileChange>,
        refs: &'a Vec<Ref>,
        ctx: Rc<AppContext>,
        head_branch_name: Option<String>,
        is_head_commit: bool,
    ) -> Self {
        Self {
            commit,
            changes,
            refs,
            ctx,
            head_branch_name,
            is_head_commit,
            pr_origin: None,
            show_actions: true,
            focus: PanelFocus::None,
        }
    }

    pub fn with_focus(mut self, focus: PanelFocus) -> Self {
        self.focus = focus;
        self
    }

    /// Attach a PR origin to the widget so the title row reads
    /// `Commit Details · PR #N` instead of just `Commit Details`.
    pub fn with_pr_origin(mut self, pr_number: Option<u64>) -> Self {
        self.pr_origin = pr_number;
        self
    }

    pub fn with_actions(mut self, show_actions: bool) -> Self {
        self.show_actions = show_actions;
        self
    }

    fn tree(&self) -> bool {
        self.ctx.ui_config.detail.file_view == DetailFileView::Tree
    }
}

/// One row of the changed-files section: a directory (`file: None`, tree
/// view only) or a file (`file: Some(index into the changes)`).
pub struct ChangeRow {
    pub file: Option<usize>,
    depth: usize,
    name: String,
}

pub fn change_path(change: &FileChange) -> &str {
    match change {
        FileChange::Add { path, .. }
        | FileChange::Modify { path, .. }
        | FileChange::Delete { path, .. } => path,
        FileChange::Move { to, .. } => to,
    }
}

#[derive(Default)]
struct DirNode {
    dirs: std::collections::BTreeMap<String, DirNode>,
    files: Vec<(String, usize)>,
}

/// Directories first, then files, each by name; a directory holding only
/// one subdirectory is merged with it into a single `a/b` row.
fn walk_dir_tree(node: &DirNode, depth: usize, out: &mut Vec<ChangeRow>) {
    for (name, mut child) in &node.dirs {
        let mut label = name.clone();
        while child.files.is_empty() && child.dirs.len() == 1 {
            let (sub_name, sub) = child.dirs.iter().next().unwrap();
            label = format!("{label}/{sub_name}");
            child = sub;
        }
        out.push(ChangeRow {
            file: None,
            depth,
            name: label,
        });
        walk_dir_tree(child, depth + 1, out);
    }
    let mut files: Vec<&(String, usize)> = node.files.iter().collect();
    files.sort();
    for (name, i) in files {
        out.push(ChangeRow {
            file: Some(*i),
            depth,
            name: name.clone(),
        });
    }
}

pub fn change_rows(changes: &[FileChange], tree: bool) -> Vec<ChangeRow> {
    if !tree {
        return changes
            .iter()
            .enumerate()
            .map(|(i, c)| ChangeRow {
                file: Some(i),
                depth: 0,
                name: change_path(c).to_string(),
            })
            .collect();
    }
    let mut root = DirNode::default();
    for (i, c) in changes.iter().enumerate() {
        let mut parts: Vec<&str> = change_path(c).split('/').collect();
        let name = parts.pop().unwrap_or_default().to_string();
        let mut node = &mut root;
        for part in parts {
            node = node.dirs.entry(part.to_string()).or_default();
        }
        node.files.push((name, i));
    }
    let mut rows = Vec::new();
    walk_dir_tree(&root, 0, &mut rows);
    rows
}

/// Reorder `changes` as the tree lists them, so index order is screen order.
pub fn sort_changes_for_tree(changes: &mut Vec<FileChange>) {
    let order: Vec<usize> = change_rows(changes, true)
        .iter()
        .filter_map(|r| r.file)
        .collect();
    let mut taken: Vec<Option<FileChange>> =
        std::mem::take(changes).into_iter().map(Some).collect();
    *changes = order.into_iter().filter_map(|i| taken[i].take()).collect();
}

/// Display label + the global `UserEvent` it dispatches. The on-screen
/// key glyph is looked up from `KeyBinds` at render time so a rebind
/// in the user's config shows up in the panel automatically, no need
/// to keep these strings in sync with `assets/default-keybind.toml`.
pub const COMMIT_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Add Tag", crate::event::UserEvent::AddTag),
    ("Blame", crate::event::UserEvent::Blame),
    ("Create Branch", crate::event::UserEvent::CreateBranch),
    ("Checkout", crate::event::UserEvent::Checkout),
    ("Cherry Pick", crate::event::UserEvent::CherryPick),
    ("Revert", crate::event::UserEvent::Revert),
    ("Drop", crate::event::UserEvent::Drop),
    ("Merge into current", crate::event::UserEvent::Merge),
    ("Rebase current on", crate::event::UserEvent::Rebase),
    ("Reset current to", crate::event::UserEvent::Reset),
    ("Squash with parent", crate::event::UserEvent::Squash),
    ("Amend", crate::event::UserEvent::AmendCommit),
];

/// Actions slice to use: HEAD commit gets Amend, others don't.
/// Squash is available for every non-initial commit (parent existence
/// checked at apply-time), so it's part of the shared prefix.
pub fn commit_actions(is_head_commit: bool) -> &'static [(&'static str, crate::event::UserEvent)] {
    if is_head_commit {
        COMMIT_ACTIONS
    } else {
        &COMMIT_ACTIONS[..11]
    }
}

/// Map a `UserEvent` to its `[scope.detail]` action name when the
/// event has no global binding (Revert today, `v` is already taken
/// globally by `clean_untracked`).
fn scoped_action_name(event: crate::event::UserEvent) -> Option<&'static str> {
    match event {
        crate::event::UserEvent::Revert => Some("revert"),
        _ => None,
    }
}

pub const STASH_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Apply Stash", crate::event::UserEvent::ApplyStash),
    ("Pop Stash", crate::event::UserEvent::PopStash),
    ("Drop Stash", crate::event::UserEvent::DropStash),
    (
        "Create Branch from Stash",
        crate::event::UserEvent::CreateBranchFromStash,
    ),
    ("Copy Stash Name", crate::event::UserEvent::CopyStashName),
    ("Copy Stash Hash", crate::event::UserEvent::CopyStashHash),
];

impl StatefulWidget for CommitDetail<'_> {
    type State = CommitDetailState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if !self.show_actions {
            self.render_panel(area, buf, state);
            return;
        }
        let [content_area, action_bar_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);

        // Left panel: top border forms the horizontal separator
        let content_block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let content_inner = content_block.inner(content_area);
        content_block.render(content_area, buf);

        // Content inner: title + underline + spacer + scrollable content
        let [content_title_area, content_underline_area, _content_spacer_area, content_scroll_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(content_inner);

        // Render centered title, appends ` · PR #N` in accent
        // colour when the commit was opened from a PR drilldown.
        let pr_suffix = self
            .pr_origin
            .map(|n| format!(" · PR #{}", n))
            .unwrap_or_default();
        let title_text = "Commit Details";
        let visible_text_len = (title_text.chars().count() + pr_suffix.chars().count()) as u16;
        let title_pad = content_title_area.width.saturating_sub(visible_text_len);
        let title_left = title_pad / 2;
        let mut title_spans: Vec<Span<'_>> = vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if !pr_suffix.is_empty() {
            title_spans.push(Span::styled(
                pr_suffix,
                Style::default()
                    .fg(self.ctx.color_theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Paragraph::new(Line::from(title_spans)).render(content_title_area, buf);

        // Render small underline (slightly shorter than title)
        let title_len = visible_text_len;
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = content_underline_area
            .width
            .saturating_sub(underline_len as u16);
        let underline_left = underline_pad / 2;
        let underline_line = Line::from(vec![
            Span::styled(" ".repeat(underline_left as usize), Style::default()),
            Span::styled(
                "─".repeat(underline_len),
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ),
        ]);
        Paragraph::new(underline_line).render(content_underline_area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)])
                .areas(content_scroll_area);

        let (mut label_lines, mut value_lines, changes_start) = self.contents(value_area.width);

        let content_area_height = content_scroll_area.height as usize;
        self.update_state(state, value_lines.len(), content_area_height);
        state.content_top = content_scroll_area.y;
        state.changes_start = changes_start;
        // File index -> line, through the rows (tree view adds directory rows).
        let rows = change_rows(self.changes, self.tree());
        let file_line = |file: usize| {
            changes_start + rows.iter().position(|r| r.file == Some(file)).unwrap_or(0)
        };
        state.ensure_selected_visible(file_line(state.selected_file));

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        // Highlight hovered file (mouse) or keyboard-selected file
        let highlight_file = state.hover_file.unwrap_or(state.selected_file);
        if !self.changes.is_empty() {
            let selected_line = file_line(highlight_file);
            let visible_selected = selected_line.saturating_sub(state.offset);
            if visible_selected < value_lines.len() {
                value_lines[visible_selected].style =
                    Style::default().add_modifier(Modifier::REVERSED);
            }
        }

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);

        if state.offset == 0 && content_scroll_area.height > 0 {
            // Overlay avatar in the value column, right before the author name
            if self.render_author_avatar(value_area, buf) {
                state.avatar_row = Some(content_scroll_area.top());
            }
        } else if state.avatar_row.is_some() {
            // Avatar scrolled out of view, schedule row-scoped deletion
            state.pending_avatar_delete_row = state.avatar_row.take();
        }
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl CommitDetail<'_> {
    /// Layout beside the diff pane: a compact header on top, the changed
    /// files below with their own scroll.
    fn render_panel(self, area: Rect, buf: &mut Buffer, state: &mut CommitDetailState) {
        let block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let inner = Block::default()
            .padding(Padding::horizontal(1))
            .inner(block.inner(area));
        block.render(area, buf);
        let width = inner.width.max(1);
        let wrapped_height = |lines: &[Line]| -> u16 {
            lines
                .iter()
                .map(|l| (l.width().max(1) as u16).div_ceil(width))
                .sum()
        };

        let info = self.compact_info_lines();
        let rows = change_rows(self.changes, self.tree());
        // The files get the rows they need; the header / message gets what
        // is left, and at least 2/5 when the files don't fit anyway.
        let body_h = inner.height.saturating_sub(1);
        let left_for_info = body_h.saturating_sub(rows.len() as u16);
        let info_h = wrapped_height(&info)
            .min(left_for_info.max((body_h * 2 / 5).max(3)))
            .min(body_h);
        let [info_area, divider_area, files_area] = Layout::vertical([
            Constraint::Length(info_h),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);

        // Explicit colour: unstyled spans inherit the block's divider colour.
        let text = Style::default().fg(self.ctx.color_theme.fg);
        Paragraph::new(info)
            .style(text)
            .wrap(Wrap { trim: false })
            .render(info_area, buf);
        // "── Changes (N) ───", accented while the files have focus.
        let theme = &self.ctx.color_theme;
        let (label_style, rule_style) = if self.focus == PanelFocus::Files {
            let accent = Style::default().fg(theme.list_head_fg);
            (accent.add_modifier(Modifier::BOLD), accent)
        } else {
            (
                Style::default().fg(theme.detail_label_fg),
                Style::default().fg(theme.divider_fg),
            )
        };
        let label = format!(" Changes ({}) ", self.changes.len());
        let rest = (divider_area.width as usize).saturating_sub(label.chars().count() + 2);
        Paragraph::new(Line::from(vec![
            Span::styled("──", rule_style),
            Span::styled(label, label_style),
            Span::styled("─".repeat(rest), rule_style),
        ]))
        .render(divider_area, buf);

        let file_line = |file: usize| rows.iter().position(|r| r.file == Some(file)).unwrap_or(0);
        let mut lines = self.changes_lines();
        self.update_state(state, lines.len(), files_area.height as usize);
        state.content_top = files_area.y;
        state.changes_start = 0;
        state.ensure_selected_visible(file_line(state.selected_file));
        let highlight_file = state.hover_file.unwrap_or(state.selected_file);
        if let Some(line) = lines.get_mut(file_line(highlight_file)) {
            if !self.changes.is_empty() {
                line.style = if self.focus == PanelFocus::Files {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().bg(theme.inactive_selected_bg())
                };
            }
        }
        let visible: Vec<Line> = lines.into_iter().skip(state.offset).collect();
        Paragraph::new(visible).style(text).render(files_area, buf);

        // No avatar in this layout: drop one placed by the wide layout.
        if let Some(row) = state.avatar_row.take() {
            state.pending_avatar_delete_row = Some(row);
        }
    }

    /// Subject, author and date, short SHA / parents / refs, then the body.
    fn compact_info_lines(&self) -> Vec<Line<'_>> {
        let mut message = self.commit_message_lines().into_iter();
        let mut lines: Vec<Line> = message.next().into_iter().collect();
        if !self.commit.author_name.is_empty() {
            let mut spans = self.author_line("").spans;
            spans.push(Span::raw("  "));
            spans.extend(self.author_date_line().spans);
            lines.push(Line::from(spans));

            let mut spans = vec![self
                .commit
                .commit_hash
                .as_short_hash()
                .fg(self.ctx.color_theme.detail_hash_fg)];
            if has_parent(self.commit) {
                spans.push(Span::raw("  parent ").fg(self.ctx.color_theme.detail_label_fg));
                spans.extend(self.parents_line().spans);
            }
            if has_refs(self.refs) {
                spans.push(Span::raw("  "));
                spans.extend(self.refs_line().spans);
            }
            lines.push(Line::from(spans));
        }
        lines.extend(message);
        lines
    }

    /// The git actions as one `key label` row, in the actions column's order.
    pub fn actions_hint_line(&self) -> Line<'static> {
        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit)
        };
        let mut spans = Vec::new();
        for (label, event) in actions {
            let key = self.action_key(*event);
            if key.is_empty() {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                key,
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(self.ctx.color_theme.detail_label_fg),
            ));
        }
        Line::from(spans)
    }

    /// Bound key of a git action. Revert uses its [scope.detail] binding
    /// (`v` is `clean_untracked` globally).
    fn action_key(&self, event: crate::event::UserEvent) -> String {
        let mut key = self.ctx.keybind.primary_global_key(event);
        if key.is_empty() {
            if let Some(scoped_name) = scoped_action_name(event) {
                key = self
                    .ctx
                    .keybind
                    .primary_scoped_key(&["detail"], scoped_name);
            }
        }
        key
    }
}

impl CommitDetail<'_> {
    fn render_labels_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::left(2)));
        paragraph.render(area, buf);
    }

    fn render_value_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::new(1, 2, 0, 0)));
        paragraph.render(area, buf);
    }

    fn render_author_avatar(&self, area: Rect, buf: &mut Buffer) -> bool {
        if area.width <= 5 || area.height == 0 {
            return false;
        }
        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return false;
        }
        avatar_manager.prefetch(
            vec![self.commit.commit_hash.as_str().to_string()],
            &self.commit.author_email,
        );
        avatar_manager.ensure_uploaded(
            &self.commit.author_email,
            1,
            false,
            self.ctx.color_theme.bg,
        );
        if let Some(prepared) = avatar_manager.prepared_image(&self.commit.author_email, 1, false) {
            for (x, image_cell) in prepared.cells().iter().enumerate() {
                let cell = &mut buf[(area.left() + x as u16 + 1, area.top())];
                cell.set_symbol(image_cell.symbol());
                cell.set_style(image_cell.style());
                cell.set_skip(image_cell.skip());
            }
            return true;
        }
        false
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &CommitDetailState) {
        // Action bar with top+left borders forming a corner with the content border
        let action_block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let inner = action_block.inner(area);
        action_block.render(area, buf);

        // Inner: title + underline + spacer + actions
        let [action_title_area, action_underline_area, _action_spacer_area, action_actions_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(inner);

        // Render centered title
        let title_text = "Git Actions";
        let title_len = title_text.chars().count() as u16;
        let title_pad = action_title_area.width.saturating_sub(title_len);
        let title_left = title_pad / 2;
        let title_line = Line::from(vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Paragraph::new(title_line).render(action_title_area, buf);

        // Render small underline
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = action_underline_area
            .width
            .saturating_sub(underline_len as u16);
        let underline_left = underline_pad / 2;
        let underline_line = Line::from(vec![
            Span::styled(" ".repeat(underline_left as usize), Style::default()),
            Span::styled(
                "─".repeat(underline_len),
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ),
        ]);
        Paragraph::new(underline_line).render(action_underline_area, buf);

        // Action content padding matching left column
        let action_pad = Block::default().padding(Padding::new(2, 1, 0, 0));
        let action_inner = action_pad.inner(action_actions_area);
        action_pad.render(action_actions_area, buf);

        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit)
        };
        let rebasing = !self.ctx.repo_path.as_os_str().is_empty()
            && crate::git::rebase::rebase_in_progress(&self.ctx.repo_path);

        let mut lines = Vec::new();
        for (i, (label, event)) in actions.iter().enumerate() {
            let is_hovered = state.hovered_action == Some(i);
            // While a rebase is in progress, the "Rebase current on" slot
            // re-labels to "Resume rebase" (yellow) and dispatches into the
            // resume view. The bound key still drives both, the action
            // just means something different in this state.
            let (effective_label, label_color) = if rebasing && *label == "Rebase current on" {
                ("Resume rebase", Some(self.ctx.color_theme.status_warn_fg))
            } else {
                (*label, None)
            };
            let mut style = if is_hovered {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            if let Some(c) = label_color {
                style = style.fg(c).add_modifier(Modifier::BOLD);
            }
            let key_style = style.add_modifier(Modifier::BOLD);
            let key = self.action_key(*event);
            let mut spans = vec![Span::styled(effective_label.to_string(), style)];
            if !key.is_empty() {
                spans.push(Span::styled(format!(" ({})", key), key_style));
            }
            lines.push(Line::from(spans));
        }

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(action_inner, buf);
    }

    fn is_stash(&self) -> bool {
        use crate::git::CommitType;
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>, usize) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        // Reserve 3 chars before the author name when avatars are on (2 cells avatar + 1 gap)
        let avatar_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let author_prefix: &'static str = if avatar_enabled { "   " } else { "" };

        label_lines.push(Line::from("   Author: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(self.author_line(author_prefix));
        label_lines.push(Line::from("     Date: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(self.author_date_line());

        if self.commit.author_name != self.commit.committer_name
            || self.commit.author_email != self.commit.committer_email
            || self.commit.author_date != self.commit.committer_date
        {
            label_lines.push(Line::from("Committer: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.committer_line());
            label_lines.push(Line::from("     Date: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.committer_date_line());
        }

        label_lines.push(Line::from("      SHA: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(self.sha_line());

        if has_parent(self.commit) {
            let parent_label = if self.commit.parent_commit_hashes.len() == 1 {
                "   Parent: "
            } else {
                "  Parents: "
            };
            label_lines.push(Line::from(parent_label).fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.parents_line());
        }

        if has_refs(self.refs) {
            label_lines.push(Line::from("     Refs: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.refs_line());
        }

        // Divider before message
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));

        label_lines.push(Line::from("  Message: ").fg(self.ctx.color_theme.detail_label_fg));
        let msg_lines = self.commit_message_lines();
        let msg_len = msg_lines.len();
        value_lines.extend(msg_lines);
        // Mirror the body lines in the left column: "Body:" on the first body
        // row, then empty labels for continuation rows so the divider below
        // stays aligned in both columns.
        if msg_len > 1 {
            label_lines.push(Line::from("     Body: ").fg(self.ctx.color_theme.detail_label_fg));
            for _ in 2..msg_len {
                label_lines.push(Line::from(""));
            }
        }

        // Divider before changes
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));
        let changes_start = value_lines.len();
        value_lines.extend(self.changes_lines());

        (label_lines, value_lines, changes_start)
    }

    fn author_line(&self, prefix: &'static str) -> Line<'_> {
        let mut spans = Vec::new();
        if !prefix.is_empty() {
            spans.push(Span::raw(prefix));
        }
        spans.push(
            self.commit
                .author_name
                .as_str()
                .fg(self.ctx.color_theme.detail_name_fg),
        );
        spans.push(" <".into());
        spans.push(
            self.commit
                .author_email
                .as_str()
                .fg(self.ctx.color_theme.detail_email_fg),
        );
        spans.push(">".into());
        Line::from(spans)
    }

    fn author_date_line(&self) -> Line<'_> {
        let date_str = self.ctx.core_config.date_time_format().format(
            &self.commit.author_date,
            self.ctx.core_config.date_time_local(),
        );
        Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg))
    }

    fn committer_line(&self) -> Line<'_> {
        Line::from(vec![
            self.commit
                .committer_name
                .as_str()
                .fg(self.ctx.color_theme.detail_name_fg),
            " <".into(),
            self.commit
                .committer_email
                .as_str()
                .fg(self.ctx.color_theme.detail_email_fg),
            ">".into(),
        ])
    }

    fn committer_date_line(&self) -> Line<'_> {
        let date_str = self.ctx.core_config.date_time_format().format(
            &self.commit.committer_date,
            self.ctx.core_config.date_time_local(),
        );
        Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg))
    }

    fn sha_line(&self) -> Line<'_> {
        Line::from(
            self.commit
                .commit_hash
                .as_str()
                .fg(self.ctx.color_theme.detail_hash_fg),
        )
    }

    fn parents_line(&self) -> Line<'_> {
        let mut spans = Vec::new();
        let parents = &self.commit.parent_commit_hashes;
        for (i, hash) in parents
            .iter()
            .map(|hash| hash.as_short_hash().fg(self.ctx.color_theme.detail_hash_fg))
            .enumerate()
        {
            spans.push(hash);
            if i < parents.len() - 1 {
                spans.push(Span::raw(" "));
            }
        }
        Line::from(spans)
    }

    fn refs_line(&self) -> Line<'_> {
        let mut compacted_branches: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut tags: Vec<String> = Vec::new();

        for r in self.refs.iter() {
            match r {
                Ref::Branch { name, .. } => {
                    compacted_branches
                        .entry(name.clone())
                        .or_default()
                        .push(name.clone());
                }
                Ref::RemoteBranch { name, .. } => {
                    if name.ends_with("/HEAD") {
                        continue;
                    }
                    let short_name = name
                        .split_once('/')
                        .map(|(_, rest)| rest.to_string())
                        .unwrap_or_else(|| name.clone());
                    compacted_branches
                        .entry(short_name)
                        .or_default()
                        .push(name.clone());
                }
                Ref::Tag { name, .. } => {
                    tags.push(name.clone());
                }
                _ => {}
            }
        }

        let mut spans = Vec::new();
        let mut first = true;

        // HEAD indicator first, like in commit list
        if let Some(ref head_name) = self.head_branch_name {
            if compacted_branches.contains_key(head_name) {
                let fg = self.ctx.color_theme.list_head_fg;
                spans.push(Span::styled(
                    "಄ ",
                    Style::default().fg(fg).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    "HEAD -> ",
                    Style::default().fg(fg).add_modifier(Modifier::BOLD),
                ));
                first = false;
            }
        }

        for (short_name, full_names) in compacted_branches.iter() {
            let is_head = self.head_branch_name.as_deref() == Some(short_name.as_str());
            if !first {
                spans.push(Span::raw(", "));
            }
            first = false;

            let is_local = full_names.iter().any(|n| !n.contains('/'));
            let remotes: Vec<&str> = full_names
                .iter()
                .filter(|n| n.contains('/'))
                .map(|n| n.split_once('/').map(|(r, _)| r).unwrap_or(n))
                .collect();

            let fg = if is_head {
                self.ctx.color_theme.list_head_fg
            } else if is_local {
                self.ctx
                    .branch_color_map
                    .get(short_name)
                    .copied()
                    .unwrap_or(self.ctx.color_theme.list_ref_branch_fg)
            } else {
                self.ctx
                    .branch_color_map
                    .get(short_name)
                    .copied()
                    .unwrap_or(self.ctx.color_theme.list_ref_remote_branch_fg)
            };

            let display = if remotes.is_empty() {
                short_name.clone()
            } else {
                format!("{}|{}", short_name, remotes.join("|"))
            };

            spans.push(Span::styled(
                "⎇ ",
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                display,
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
        }

        for tag_name in tags.iter() {
            if !first {
                spans.push(Span::raw(", "));
            }
            first = false;
            let fg = self.ctx.color_theme.list_ref_tag_fg;
            spans.push(Span::styled(
                "🏷  ",
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                tag_name.clone(),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
        }

        Line::from(spans)
    }

    fn commit_message_lines(&self) -> Vec<Line<'_>> {
        let commit_message_line = Line::from(self.commit.commit_message.as_str().bold());
        let mut lines = vec![commit_message_line];
        if !self.commit.body.is_empty() {
            lines.extend(self.commit.body.lines().map(Line::raw));
        }
        lines
    }

    fn changes_lines(&self) -> Vec<Line<'_>> {
        change_rows(self.changes, self.tree())
            .into_iter()
            .map(|row| {
                let indent = Span::raw("  ".repeat(row.depth));
                let Some(file) = row.file else {
                    return Line::from(vec![
                        indent,
                        Span::styled(
                            format!("▾ {}/", row.name),
                            Style::default().fg(self.ctx.color_theme.detail_label_fg),
                        ),
                    ]);
                };
                let c = &self.changes[file];
                let path = row.name;
                let (status, add, del) = match c {
                    FileChange::Add { additions, .. } => ("A", *additions, 0usize),
                    FileChange::Modify {
                        additions,
                        deletions,
                        ..
                    } => ("M", *additions, *deletions),
                    FileChange::Delete { deletions, .. } => ("D", 0usize, *deletions),
                    FileChange::Move {
                        additions,
                        deletions,
                        ..
                    } => ("R", *additions, *deletions),
                };
                let add_str = if add > 0 {
                    format!(" +{add}")
                } else {
                    "".to_string()
                };
                let del_str = if del > 0 {
                    format!(" -{del}")
                } else {
                    "".to_string()
                };
                let status_color = match status {
                    "A" => self.ctx.color_theme.detail_file_change_add_fg,
                    "M" => self.ctx.color_theme.detail_file_change_modify_fg,
                    "D" => self.ctx.color_theme.detail_file_change_delete_fg,
                    _ => self.ctx.color_theme.detail_file_change_move_fg,
                };
                let is_deleted = matches!(c, FileChange::Delete { .. });
                let path_style = if is_deleted {
                    Style::default()
                        .fg(self.ctx.color_theme.fg)
                        .add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::default().fg(self.ctx.color_theme.fg)
                };
                Line::from(vec![
                    indent,
                    Span::styled(format!("{} ", status), Style::default().fg(status_color)),
                    Span::styled(path, path_style),
                    add_str.fg(self.ctx.color_theme.detail_file_change_add_fg),
                    del_str.fg(self.ctx.color_theme.detail_file_change_delete_fg),
                ])
            })
            .collect()
    }

    fn divider_line(&self, width: usize) -> Line<'_> {
        Line::from("─".repeat(width).fg(self.ctx.color_theme.divider_fg))
    }

    fn update_state(&self, state: &mut CommitDetailState, line_count: usize, area_height: usize) {
        state.height = area_height;
        state.offset = state.offset.min(line_count.saturating_sub(area_height));
    }
}

fn has_parent(commit: &Commit) -> bool {
    !commit.parent_commit_hashes.is_empty()
}

fn has_refs(refs: &[Ref]) -> bool {
    refs.iter().any(|r| {
        matches!(
            r,
            Ref::Branch { .. } | Ref::RemoteBranch { .. } | Ref::Tag { .. }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn test_commit() -> Commit {
        let date = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 5, 7, 20, 39, 0)
            .unwrap();
        Commit {
            commit_hash: "281d15fa8de043a2ee72d0971f5739ebf4335ba6".into(),
            author_name: "Alice".into(),
            author_email: "alice@example.com".into(),
            author_date: date,
            committer_name: "Alice".into(),
            committer_email: "alice@example.com".into(),
            committer_date: date,
            commit_message: "feat: fixed author header".into(),
            body: String::new(),
            parent_commit_hashes: vec![],
            commit_type: crate::git::CommitType::Commit,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<&str>>()
            .join("")
    }

    #[test]
    fn scrollable_contents_include_author_and_identity() {
        let commit = test_commit();
        let changes = Vec::new();
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None, false);

        let (label_lines, value_lines, _) = detail.contents(80);
        let labels = label_lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        let values = value_lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(labels.contains("Author"));
        assert!(values.contains("Alice"));
        assert!(values.contains("alice@example.com"));
    }

    fn modify(path: &str) -> FileChange {
        FileChange::Modify {
            path: path.into(),
            additions: 1,
            deletions: 1,
        }
    }

    #[test]
    fn tree_rows_list_dirs_first_and_merge_single_child_dirs() {
        let mut changes = vec![
            modify("README.md"),
            modify("src/a/b/deep.c"),
            modify("src/main.c"),
            modify("include/x.h"),
        ];
        sort_changes_for_tree(&mut changes);
        let rows: Vec<(Option<usize>, usize, String)> = change_rows(&changes, true)
            .into_iter()
            .map(|r| (r.file, r.depth, r.name))
            .collect();
        assert_eq!(
            rows,
            vec![
                (None, 0, "include".into()),
                (Some(0), 1, "x.h".into()),
                (None, 0, "src".into()),
                (None, 1, "a/b".into()),
                (Some(1), 2, "deep.c".into()),
                (Some(2), 1, "main.c".into()),
                (Some(3), 0, "README.md".into()),
            ]
        );
        // Sorted changes follow the rows, so file index order is screen order.
        assert_eq!(change_path(&changes[1]), "src/a/b/deep.c");
    }

    #[test]
    fn scrollable_contents_start_with_author_line() {
        let commit = test_commit();
        let changes = Vec::new();
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None, false);

        let (label_lines, _, _) = detail.contents(80);
        assert_eq!(line_text(&label_lines[0]).trim(), "Author:");
    }
}
