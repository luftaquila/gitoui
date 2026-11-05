use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    config::DetailFileView,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    git::{diff::DiffEntry, Commit, CommitHash, CommitType, FileChange, Ref, Repository},
    view::{
        diff::DiffView,
        workspace::{main_areas, MainAreas},
        ListRefreshViewContext, RefreshViewContext,
    },
    widget::{
        commit_detail::{
            change_path, change_rows, sort_changes_for_tree, CommitDetail, CommitDetailState,
        },
        commit_list::{CommitList, CommitListState},
    },
};

/// Commit list on top, detail panel below. Shared by the Detail view and
/// the List view's preview, so switching keeps the list in place.
pub(crate) fn split_list_detail(ctx: &AppContext, area: Rect) -> [Rect; 2] {
    let detail_height =
        crate::view::adaptive_detail_height(area.height, ctx.ui_config.detail.height, 5);
    Layout::vertical([Constraint::Min(0), Constraint::Length(detail_height)]).areas(area)
}

/// Delete Kitty graph placements left on rows the moved panel now covers.
pub(crate) fn clear_rows_under(ctx: &AppContext, area: Rect, cleared: &mut Option<Rect>) {
    if *cleared != Some(area) {
        for y in area.top()..area.bottom() {
            let _ = ctx.image_protocol.delete_row(y);
        }
        *cleared = Some(area);
    }
}

/// Changes in the order the configured file view lists them.
pub(crate) fn arrange_changes(ctx: &AppContext, mut changes: Vec<FileChange>) -> Vec<FileChange> {
    if ctx.ui_config.detail.file_view == DetailFileView::Tree {
        sort_changes_for_tree(&mut changes);
    }
    changes
}

fn file_at_line(
    ctx: &AppContext,
    changes: &[FileChange],
    line: usize,
    start: usize,
) -> Option<usize> {
    let tree = ctx.ui_config.detail.file_view == DetailFileView::Tree;
    change_rows(changes, tree)
        .get(line.checked_sub(start)?)
        .and_then(|r| r.file)
}

/// The selected file's diff beside the changed files (`[ui.detail] diff_pane`).
#[derive(Debug, Default)]
pub(crate) struct DiffPane<'a> {
    /// Commit and file index the view below was built for.
    key: Option<(CommitHash, usize)>,
    view: Option<DiffView<'a>>,
    /// Where the last render drew it.
    area: Option<Rect>,
}

impl<'a> DiffPane<'a> {
    /// Rebuild the diff when the commit or the file changed. For the
    /// uncommitted row it is index + work tree against HEAD.
    fn sync(
        &mut self,
        ctx: &Rc<AppContext>,
        tx: &Sender,
        commit: &Commit,
        changes: &[FileChange],
        selected: usize,
    ) {
        let key = (commit.commit_hash.clone(), selected);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        self.view = changes.get(selected).and_then(|change| {
            let path = change_path(change);
            if crate::git::diff::is_binary_extension(path) {
                return None;
            }
            let uncommitted = commit.commit_type == CommitType::Uncommitted;
            let (entry, hash) = if uncommitted {
                let entry = match change {
                    FileChange::Add { .. } => DiffEntry::load_untracked_file(&ctx.repo_path, path)
                        .ok()
                        .or_else(|| {
                            DiffEntry::load_combined_uncommitted_for_file(&ctx.repo_path, path, 3)
                                .ok()
                                .flatten()
                        }),
                    _ => DiffEntry::load_combined_uncommitted_for_file(&ctx.repo_path, path, 3)
                        .ok()
                        .flatten(),
                }?;
                (entry, String::new())
            } else {
                let hash = commit.commit_hash.as_str();
                let entry = DiffEntry::load_for_file(&ctx.repo_path, hash, path).ok()?;
                (entry, hash.to_string())
            };
            let all_files = changes
                .iter()
                .map(|c| (change_path(c).to_string(), false))
                .collect();
            Some(DiffView::new(
                None,
                vec![entry],
                ctx.clone(),
                tx.clone(),
                format!("Diff: {path}"),
                hash,
                all_files,
                ctx.repo_path.clone(),
            ))
        });
    }

    fn render(&mut self, f: &mut Frame, area: Rect) {
        self.area = Some(area);
        if let Some(view) = self.view.as_mut() {
            view.render(f, area);
        }
    }

    fn scroll(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        if let Some(view) = self.view.as_mut() {
            view.handle_event(event_with_count, key);
        }
    }
}

/// Wheel over a detail panel scrolls the part under the pointer.
fn wheel_panel(
    col: u16,
    row: u16,
    down: bool,
    info_area: Option<Rect>,
    state: &mut CommitDetailState,
    diff: &mut DiffPane<'_>,
) -> bool {
    use ratatui::{
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        layout::Position,
    };
    let pos = Position::new(col, row);
    if diff.area.is_some_and(|a| a.contains(pos)) {
        let (event, code) = if down {
            (UserEvent::ScrollDown, KeyCode::Down)
        } else {
            (UserEvent::ScrollUp, KeyCode::Up)
        };
        diff.scroll(
            UserEventWithCount::new(event, 3),
            KeyEvent::new(code, KeyModifiers::NONE),
        );
        return true;
    }
    if info_area.is_some_and(|a| a.contains(pos)) {
        for _ in 0..3 {
            if down {
                state.scroll_down();
            } else {
                state.scroll_up();
            }
        }
        return true;
    }
    false
}

/// `[commit info / files, diff]` of a detail panel with the diff pane.
pub(crate) fn split_panel(area: Rect) -> [Rect; 2] {
    Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area)
}

/// Commit info and files, plus the selected file's diff with `diff_pane`.
/// Returns the info / files area.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_detail_panel(
    f: &mut Frame,
    area: Rect,
    ctx: &Rc<AppContext>,
    tx: &Sender,
    widget: CommitDetail<'_>,
    commit: &Commit,
    changes: &[FileChange],
    state: &mut CommitDetailState,
    diff: &mut DiffPane<'_>,
) -> Rect {
    if !ctx.ui_config.detail.diff_pane {
        diff.area = None;
        f.render_stateful_widget(widget, area, state);
        return area;
    }
    // Git actions as one row across the panel's bottom.
    let hint = widget.actions_hint_line();
    let [main_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let [info_area, diff_area] = split_panel(main_area);
    f.render_stateful_widget(widget.with_actions(false), info_area, state);
    f.render_widget(
        ratatui::widgets::Paragraph::new(hint).block(
            ratatui::widgets::Block::default().padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        hint_area,
    );
    diff.sync(ctx, tx, commit, changes, state.selected_file);
    diff.render(f, diff_area);
    info_area
}

/// The `[ui.detail] preview` panel's content for the highlighted row.
#[derive(Debug)]
pub(crate) struct DetailPreview<'a> {
    pub commit: Commit,
    changes: Vec<FileChange>,
    refs: Vec<Ref>,
    head_branch_name: Option<String>,
    is_head_commit: bool,
    state: CommitDetailState,
    diff: DiffPane<'a>,
    info_area: Option<Rect>,
}

impl<'a> DetailPreview<'a> {
    pub fn new(
        ctx: &AppContext,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        head_branch_name: Option<String>,
        head_commit_hash: Option<CommitHash>,
    ) -> Self {
        let is_head_commit = head_commit_hash.as_ref() == Some(&commit.commit_hash);
        DetailPreview {
            commit,
            changes: arrange_changes(ctx, changes),
            refs,
            head_branch_name,
            is_head_commit,
            state: CommitDetailState::default(),
            diff: DiffPane::default(),
            info_area: None,
        }
    }

    /// Preview of the uncommitted row (`hash` is its id), diffs against HEAD.
    pub fn uncommitted(
        ctx: &AppContext,
        hash: CommitHash,
        changes: &crate::git::status::UncommittedChanges,
    ) -> Self {
        use crate::git::status::StatusType;
        let mut files: Vec<FileChange> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in changes.staged.iter().chain(&changes.unstaged) {
            if !seen.insert(f.path.clone()) {
                continue;
            }
            let path = f.path.clone();
            files.push(match f.status {
                StatusType::Added => FileChange::Add { path, additions: 0 },
                StatusType::Deleted => FileChange::Delete { path, deletions: 0 },
                StatusType::Renamed => FileChange::Move {
                    from: f.old_path.clone().unwrap_or_default(),
                    to: path,
                    additions: 0,
                    deletions: 0,
                },
                _ => FileChange::Modify {
                    path,
                    additions: 0,
                    deletions: 0,
                },
            });
        }
        for f in &changes.untracked {
            if seen.insert(f.path.clone()) {
                files.push(FileChange::Add {
                    path: f.path.clone(),
                    additions: 0,
                });
            }
        }
        let commit = Commit {
            commit_hash: hash,
            commit_message: format!("Uncommitted Changes ({})", files.len()),
            commit_type: CommitType::Uncommitted,
            ..Commit::default()
        };
        Self::new(ctx, commit, files, Vec::new(), None, None)
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Rc<AppContext>, tx: &Sender) {
        let widget = CommitDetail::new(
            &self.commit,
            &self.changes,
            &self.refs,
            ctx.clone(),
            self.head_branch_name.clone(),
            self.is_head_commit,
        );
        self.info_area = Some(render_detail_panel(
            f,
            area,
            ctx,
            tx,
            widget,
            &self.commit,
            &self.changes,
            &mut self.state,
            &mut self.diff,
        ));
    }

    pub fn handle_wheel(&mut self, col: u16, row: u16, down: bool) -> bool {
        wheel_panel(
            col,
            row,
            down,
            self.info_area,
            &mut self.state,
            &mut self.diff,
        )
    }
}

#[derive(Debug)]
pub struct DetailView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    commit_detail_state: CommitDetailState,

    commit: Commit,
    changes: Vec<FileChange>,
    refs: Vec<Ref>,
    head_branch_name: Option<String>,
    head_commit_hash: Option<CommitHash>,
    /// PR number the user is currently exploring, surfaces in the
    /// "Commit Details" panel title so they always know the origin.
    pr_origin: Option<u64>,

    ctx: Rc<AppContext>,
    tx: Sender,
    list_height: usize,
    detail_area: Option<Rect>,
    /// Info / files part of `detail_area` (all of it without the diff pane).
    info_area: Option<Rect>,
    diff_pane: DiffPane<'a>,
    cleared_detail_area: Option<Rect>,
}

impl<'a> DetailView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        head_branch_name: Option<String>,
        head_commit_hash: Option<CommitHash>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> DetailView<'a> {
        DetailView {
            commit_list_state: Some(commit_list_state),
            commit_detail_state: CommitDetailState::default(),
            commit,
            changes: arrange_changes(&ctx, changes),
            refs,
            head_branch_name,
            head_commit_hash,
            pr_origin: None,
            ctx,
            tx,
            list_height: 0,
            detail_area: None,
            info_area: None,
            diff_pane: DiffPane::default(),
            cleared_detail_area: None,
        }
    }

    pub fn set_pr_origin(&mut self, pr_number: Option<u64>) {
        self.pr_origin = pr_number;
    }

    /// Pre-select the file whose path matches in the commit's change list.
    /// Used when arriving from Blame / FileHistory so the user lands directly
    /// on the file they were inspecting, `ensure_selected_visible` (run on
    /// the next render) scrolls the file row into view automatically. Matches
    /// against every variant of `FileChange` including the `from`/`to` sides
    /// of a Move so renames in either direction still resolve.
    pub fn select_file_by_path(&mut self, target: &str) {
        let idx = self.changes.iter().position(|c| match c {
            FileChange::Add { path, .. }
            | FileChange::Modify { path, .. }
            | FileChange::Delete { path, .. } => path == target,
            FileChange::Move { from, to, .. } => to == target || from == target,
        });
        if let Some(i) = idx {
            self.commit_detail_state.selected_file = i;
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        // Per-view shortcut: `revert` is scoped to [scope.detail] because
        // `v` is taken globally by `clean_untracked` (uncommitted view).
        // Going through the scope resolver, instead of a raw-key
        // intercept, means a user rebinding `[keybind.detail] revert`
        // works without any extra wiring, and the action panel can
        // surface the actual bound key.
        if let Some("revert") = self.ctx.keybind.resolve_scoped(&["detail"], key) {
            self.tx.send(AppEvent::OpenDialog(DialogKind::Revert {
                target: self.commit.commit_hash.as_str().into(),
            }));
            return;
        }

        let diff_pane = self.ctx.ui_config.detail.diff_pane;

        // With the diff pane, paging scrolls the diff; the file list
        // follows the selection.
        if diff_pane
            && matches!(
                event,
                UserEvent::PageDown
                    | UserEvent::PageUp
                    | UserEvent::HalfPageDown
                    | UserEvent::HalfPageUp
                    | UserEvent::ScrollDown
                    | UserEvent::ScrollUp
            )
        {
            self.diff_pane.scroll(event_with_count, key);
            return;
        }

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    self.commit_detail_state
                        .select_next_file(self.changes.len());
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.commit_detail_state.select_prev_file();
                }
            }
            UserEvent::NavigateRight => {
                self.tx.send(AppEvent::SelectOlderCommit);
            }
            UserEvent::NavigateLeft => {
                self.tx.send(AppEvent::SelectNewerCommit);
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_page_down();
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_page_up();
                }
            }
            UserEvent::HalfPageDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_half_page_down();
                }
            }
            UserEvent::HalfPageUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_half_page_up();
                }
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_down();
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_up();
                }
            }
            UserEvent::GoToTop => {
                self.commit_detail_state.select_first();
            }
            UserEvent::GoToBottom => {
                self.commit_detail_state.select_last();
            }
            UserEvent::SelectDown => {
                self.tx.send(AppEvent::SelectOlderCommit);
            }
            UserEvent::SelectUp => {
                self.tx.send(AppEvent::SelectNewerCommit);
            }
            UserEvent::GoToParent => {
                self.tx.send(AppEvent::SelectParentCommit);
            }
            UserEvent::ShortCopy => {
                self.copy_commit_hash();
            }
            UserEvent::FullCopy => {
                self.copy_commit_message();
            }
            UserEvent::UserCommand(n) => {
                self.tx.send(AppEvent::OpenUserCommand(n));
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Confirm => {
                self.open_selected_file_diff();
            }
            UserEvent::Blame => {
                // Blame the currently-highlighted file in the commit's file list.
                if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                    let path = match change {
                        FileChange::Add { path, .. }
                        | FileChange::Modify { path, .. }
                        | FileChange::Delete { path, .. } => path.clone(),
                        FileChange::Move { to, .. } => to.clone(),
                    };
                    self.tx.send(AppEvent::OpenBlame { file_path: path });
                }
            }
            UserEvent::FileHistory => {
                if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                    let path = match change {
                        FileChange::Add { path, .. }
                        | FileChange::Modify { path, .. }
                        | FileChange::Delete { path, .. } => path.clone(),
                        FileChange::Move { to, .. } => to.clone(),
                    };
                    self.tx.send(AppEvent::OpenFileHistory { file_path: path });
                }
            }
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            UserEvent::AddTag => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::AddTag {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::CreateBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Checkout => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: self.commit.commit_hash.as_str().into(),
                    is_branch: false,
                }));
            }
            UserEvent::CherryPick => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Revert => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Revert {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Drop => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Drop {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Merge => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: self.commit.commit_hash.as_str().into(),
                    is_branch: false,
                }));
            }
            UserEvent::Rebase => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Reset => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Reset {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Squash => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Squash {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::ApplyStash => {
                self.tx.send(AppEvent::ExecuteGitAction {
                    target: self.commit.commit_hash.as_str().into(),
                    action: crate::event::GitAction::ApplyStash,
                });
            }
            UserEvent::PopStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash {
                            stash_ref,
                        }));
                }
            }
            UserEvent::DropStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash {
                            stash_ref,
                        }));
                }
            }
            UserEvent::CreateBranchFromStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
                            target: self.commit.commit_hash.as_str().into(),
                            stash_ref,
                        }));
                }
            }
            UserEvent::CopyStashName => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.copy_to_clipboard("Stash name".into(), stash_ref);
                }
            }
            UserEvent::CopyStashHash => {
                self.copy_commit_hash();
            }
            UserEvent::AbortOperation => {
                self.tx.send(AppEvent::CheckAbortOperation);
            }
            UserEvent::AmendCommit => {
                if self.is_head_commit() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::AmendMessage {
                        current_message: self.commit.commit_message.clone(),
                    }));
                } else {
                    self.tx.send(AppEvent::NotifyWarn(
                        "Amend is only available for the HEAD commit".into(),
                    ));
                }
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let areas = self.layout(area);
        let detail_area = areas.panel.unwrap_or(areas.list);
        self.detail_area = Some(detail_area);

        let ctx = self.ctx.clone();
        let list_state = self.commit_list_state.as_mut().unwrap();
        let commit_list = CommitList::new(ctx.clone());
        f.render_stateful_widget(commit_list, areas.list, list_state);

        let commit_detail = CommitDetail::new(
            &self.commit,
            &self.changes,
            &self.refs,
            self.ctx.clone(),
            self.head_branch_name.clone(),
            self.is_head_commit(),
        )
        .with_pr_origin(self.pr_origin);
        let info_area = render_detail_panel(
            f,
            detail_area,
            &self.ctx,
            &self.tx,
            commit_detail,
            &self.commit,
            &self.changes,
            &mut self.commit_detail_state,
            &mut self.diff_pane,
        );
        self.info_area = Some(info_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let areas = self.layout(area);
        let list_area = areas.list;
        if let Some(detail_area) = areas.panel {
            clear_rows_under(&self.ctx, detail_area, &mut self.cleared_detail_area);
        }
        self.list_height = list_area.height as usize;
        self.as_mut_list_state()
            .update_height(list_area.height as usize);
    }

    pub fn prepare_graph_uploads(&mut self) {
        self.as_mut_list_state().ensure_visible_graph_uploaded();
        let ctx = self.ctx.clone();
        self.as_mut_list_state().ensure_visible_avatars_uploaded(
            &mut ctx.avatar_manager.lock().unwrap(),
            ctx.color_theme.bg,
            ctx.color_theme.list_selected_bg,
        );
    }

    pub fn clear_graph_images(&mut self) {
        self.as_mut_list_state().clear_graph_images();
    }
}

impl<'a> DetailView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        self.commit_list_state.take().unwrap()
    }

    pub fn as_mut_list_state(&mut self) -> &mut CommitListState<'a> {
        self.commit_list_state.as_mut().unwrap()
    }

    pub fn as_list_state(&self) -> &CommitListState<'a> {
        self.commit_list_state.as_ref().unwrap()
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.as_mut_list_state().drain_pending_graph_uploads()
    }

    pub fn drain_pending_avatar_deletes(&mut self) -> Vec<u16> {
        self.commit_detail_state
            .drain_pending_avatar_delete()
            .into_iter()
            .collect()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    /// The List view's layout with its preview panel.
    fn layout(&self, area: Rect) -> MainAreas {
        main_areas(&self.ctx, area, true)
    }

    pub fn select_older_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_next());
    }

    pub fn select_newer_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_prev());
    }

    pub fn select_parent_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_parent());
    }

    fn update_selected_commit<F>(&mut self, repository: &Repository, update_commit_list_state: F)
    where
        F: FnOnce(&mut CommitListState<'a>),
    {
        let commit_list_state = self.as_mut_list_state();
        update_commit_list_state(commit_list_state);
        let selected = commit_list_state.selected_commit_hash().clone();
        let (commit, changes) = repository.commit_detail(&selected);
        let refs = repository.refs(&selected).into_iter().cloned().collect();
        self.commit = commit;
        self.changes = arrange_changes(&self.ctx, changes);
        self.refs = refs;

        self.commit_detail_state.select_first();
    }

    fn copy_commit_hash(&self) {
        let selected = &self.commit.commit_hash;
        self.copy_to_clipboard("Commit SHA".into(), selected.as_str().into());
    }

    fn copy_commit_message(&self) {
        self.copy_to_clipboard("Commit message".into(), self.commit.commit_message.clone());
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::Detail { list_context };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let row = row as usize;
        let Some(detail_area) = self.detail_area else {
            return;
        };
        let detail_y = detail_area.y as usize;

        if row < detail_y {
            // Click in commit list pane, ignore
            return;
        }

        if self.ctx.ui_config.detail.diff_pane {
            // Diff pane on the right: nothing clickable there yet.
            if self.info_area.is_some_and(|a| col >= a.right()) {
                return;
            }
        } else {
            // Check if click is in action bar area (right 40%)
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x {
                let action_bar_row = (row - detail_y).saturating_sub(4);
                if let Some(action_idx) = self.action_index_at_row(action_bar_row) {
                    self.execute_action(action_idx);
                }
                return;
            }
        }

        if let Some(file_idx) = self.file_at_row(row as u16) {
            self.commit_detail_state.selected_file = file_idx;
            // With the diff pane the click already shows the diff beside.
            if !self.ctx.ui_config.detail.diff_pane {
                self.open_selected_file_diff();
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // Local helper: clear hover, but BEFORE clearing copy the
        // last hovered index into `selected_file` so the highlight
        // sticks where the user last pointed instead of snapping
        // back to file 0. Without this the left pane visually
        // "resets" every time the cursor leaves the file column.
        let freeze_selection = |state: &mut crate::widget::commit_detail::CommitDetailState| {
            if let Some(idx) = state.hover_file.take() {
                state.selected_file = idx;
            }
        };

        let row = row as usize;
        let Some(detail_area) = self.detail_area else {
            freeze_selection(&mut self.commit_detail_state);
            return;
        };
        let detail_y = detail_area.y as usize;

        if row < detail_y {
            freeze_selection(&mut self.commit_detail_state);
            self.commit_detail_state.hovered_action = None;
            return;
        }

        // Check if hover is in action bar area (right 40%), or in the diff
        // pane when that replaces the action bar.
        let action_bar_x = if self.ctx.ui_config.detail.diff_pane {
            self.info_area
                .map(|a| a.right())
                .unwrap_or(detail_area.right())
        } else {
            detail_area.x + (detail_area.width as f32 * 0.6) as u16
        };
        if col >= action_bar_x && self.ctx.ui_config.detail.diff_pane {
            freeze_selection(&mut self.commit_detail_state);
            self.commit_detail_state.hovered_action = None;
            return;
        }
        if col >= action_bar_x {
            let action_bar_row = (row - detail_y).saturating_sub(4);
            self.commit_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            freeze_selection(&mut self.commit_detail_state);
            return;
        } else {
            self.commit_detail_state.hovered_action = None;
        }

        if let Some(file_idx) = self.file_at_row(row as u16) {
            self.commit_detail_state.hover_file = Some(file_idx);
        } else {
            freeze_selection(&mut self.commit_detail_state);
        }
    }

    /// Wheel scrolls the panel part under the pointer; false off-panel.
    pub fn handle_wheel(&mut self, col: u16, row: u16, down: bool) -> bool {
        wheel_panel(
            col,
            row,
            down,
            self.info_area,
            &mut self.commit_detail_state,
            &mut self.diff_pane,
        )
    }

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        use crate::widget::commit_detail::{commit_actions, STASH_ACTIONS};
        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit())
        };
        if action_bar_row < actions.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        use crate::widget::commit_detail::{commit_actions, STASH_ACTIONS};
        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit())
        };
        if action_idx >= actions.len() {
            return;
        }
        let hash = self.commit.commit_hash.as_str().into();
        if self.is_stash() {
            match action_idx {
                0 => self.tx.send(AppEvent::ExecuteGitAction {
                    target: hash,
                    action: crate::event::GitAction::ApplyStash,
                }),
                1 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash {
                                stash_ref,
                            }));
                    }
                }
                2 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash {
                                stash_ref,
                            }));
                    }
                }
                3 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
                                target: hash,
                                stash_ref,
                            }));
                    }
                }
                4 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.copy_to_clipboard("Stash name".into(), stash_ref);
                    }
                }
                5 => self.copy_commit_hash(),
                _ => {}
            }
        } else {
            // Indices must stay in sync with `COMMIT_ACTIONS` in
            // `src/widget/commit_detail.rs`.
            match action_idx {
                0 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::AddTag { target: hash })),
                1 => {
                    // Blame the currently-highlighted file in the commit's
                    // change list. Same logic as the `b` keybind handler.
                    if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                        let path = match change {
                            FileChange::Add { path, .. }
                            | FileChange::Modify { path, .. }
                            | FileChange::Delete { path, .. } => path.clone(),
                            FileChange::Move { to, .. } => to.clone(),
                        };
                        self.tx.send(AppEvent::OpenBlame { file_path: path });
                    }
                }
                2 => self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch {
                    target: hash,
                })),
                3 => self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: hash,
                    is_branch: false,
                })),
                4 => self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick {
                    target: hash,
                })),
                5 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Revert { target: hash })),
                6 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Drop { target: hash })),
                7 => self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: hash,
                    is_branch: false,
                })),
                8 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Rebase { target: hash })),
                9 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Reset { target: hash })),
                10 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Squash { target: hash })),
                11 => {
                    if self.is_head_commit() {
                        self.tx.send(AppEvent::OpenDialog(DialogKind::AmendMessage {
                            current_message: self.commit.commit_message.clone(),
                        }));
                    } else {
                        self.tx.send(AppEvent::NotifyWarn(
                            "Amend is only available for the HEAD commit".into(),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    fn is_head_commit(&self) -> bool {
        if let Some(ref head_hash) = self.head_commit_hash {
            return *head_hash == self.commit.commit_hash;
        }
        // Fallback: check if any branch ref on this commit matches head_branch_name
        if let Some(ref head_name) = self.head_branch_name {
            return self.refs.iter().any(|r| {
                if let Ref::Branch { name, .. } = r {
                    name == head_name
                } else {
                    false
                }
            });
        }
        false
    }

    fn is_stash(&self) -> bool {
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn stash_ref(&self) -> Option<String> {
        self.refs.iter().find_map(|r| {
            if let Ref::Stash { name, .. } = r {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    fn open_selected_file_diff(&self) {
        if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
            match change {
                FileChange::Add { path, .. } | FileChange::Modify { path, .. } => {
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: path.clone(),
                    });
                }
                FileChange::Move { to, .. } => {
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: to.clone(),
                    });
                }
                FileChange::Delete { path, .. } => {
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: path.clone(),
                    });
                }
            }
        }
    }

    /// Changed file at screen `row`, from the rows the last render laid out.
    fn file_at_row(&self, row: u16) -> Option<usize> {
        let state = &self.commit_detail_state;
        let line = state.content_line_at(row)?;
        file_at_line(&self.ctx, &self.changes, line, state.changes_start())
    }
}
