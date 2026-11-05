use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    config::RefsPosition,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    git::{actions, Ref},
    view::{
        workspace::{main_areas, preview_on},
        ListRefreshViewContext, RefreshViewContext, RefsRefreshViewContext,
    },
    widget::{
        commit_list::{CommitList, CommitListState},
        ref_list::{RefList, RefListState},
    },
};

#[derive(Debug)]
pub struct RefsView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    panel: RefsPanel,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> RefsView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> RefsView<'a> {
        let mut commit_list_state = commit_list_state;
        // Reuse the always-open sidebar's tree (selection, open nodes).
        let panel = commit_list_state
            .workspace
            .refs
            .take()
            .unwrap_or_else(|| RefsPanel::load(refs));
        RefsView {
            commit_list_state: Some(commit_list_state),
            panel,
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::Quit => {
                self.tx.send(AppEvent::Quit);
            }
            UserEvent::Cancel | UserEvent::Close | UserEvent::RefList => {
                self.tx.send(AppEvent::CloseRefs);
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                for _ in 0..count {
                    self.panel.state.select_next();
                }
                self.update_commit_list_selected();
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                for _ in 0..count {
                    self.panel.state.select_prev();
                }
                self.update_commit_list_selected();
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.panel.state.scroll_down(3);
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.panel.state.scroll_up(3);
                }
            }
            UserEvent::GoToTop => {
                self.panel.state.select_first();
                self.update_commit_list_selected();
            }
            UserEvent::GoToBottom => {
                self.panel.state.select_last();
                self.update_commit_list_selected();
            }
            UserEvent::NavigateRight => {
                self.panel.state.open_node();
                self.update_commit_list_selected();
            }
            UserEvent::NavigateLeft => {
                self.panel.state.close_node();
                self.update_commit_list_selected();
            }
            UserEvent::Confirm => {
                if self.panel.state.selected_is_add_remote_item() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::AddRemote));
                } else if self.panel.state.selected_is_add_worktree_item() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::AddWorktree));
                } else if self.panel.state.selected_is_node() {
                    self.panel.state.toggle_selected();
                    self.update_commit_list_selected();
                } else if let Some(wt_path) = self.panel.state.selected_worktree_path() {
                    let display_name = self
                        .panel
                        .worktrees
                        .iter()
                        .find(|wt| wt.path == wt_path)
                        .and_then(|wt| wt.branch.as_deref())
                        .and_then(|b| b.strip_prefix("refs/heads/"))
                        .unwrap_or(&wt_path)
                        .to_string();
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmSwitchWorktree {
                            path: wt_path,
                            display_name,
                        }));
                } else if let Some(branch_name) = self.panel.state.selected_branch() {
                    self.tx.send(AppEvent::OpenBranchDetail { branch_name });
                } else if let Some(tag_name) = self.panel.state.selected_tag() {
                    self.tx.send(AppEvent::OpenTagDetail { tag_name });
                } else if let Some(stash_ref) = self.panel.state.selected_stash_ref() {
                    self.tx.send(AppEvent::OpenStashDiff { stash_ref });
                } else {
                    self.panel.state.toggle_selected();
                    self.update_commit_list_selected();
                }
            }
            UserEvent::DeleteBranch => {
                if let Some(wt_path) = self.panel.state.selected_worktree_path() {
                    if let Some(wt) = self.panel.worktrees.iter().find(|w| w.path == wt_path) {
                        if wt.is_current {
                            self.tx.send(AppEvent::NotifyError(
                                "Cannot remove the currently active worktree.".into(),
                            ));
                        } else {
                            let display_name = wt
                                .branch
                                .as_deref()
                                .and_then(|b| b.strip_prefix("refs/heads/"))
                                .unwrap_or(&wt.path)
                                .to_string();
                            self.tx
                                .send(AppEvent::OpenDialog(DialogKind::ConfirmDeleteWorktree {
                                    path: wt_path,
                                    display_name,
                                    is_dirty: wt.is_dirty,
                                }));
                        }
                    }
                }
            }
            UserEvent::ShortCopy | UserEvent::FullCopy => {
                self.copy_ref_name();
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let areas = main_areas(&self.ctx, area, Some(&self.panel), preview_on(&self.ctx));
        let ctx = self.ctx.clone();
        let tx = self.tx.clone();
        let list_state = self.commit_list_state.as_mut().unwrap();
        // The tree has focus; the list and preview stay on screen.
        let commit_list = CommitList::new(ctx.clone()).with_focus(Some(false));
        f.render_stateful_widget(commit_list, areas.list, list_state);
        if let Some(panel_area) = areas.panel {
            list_state
                .workspace
                .render_preview(f, panel_area, &ctx, &tx);
        }
        if let Some(refs_area) = areas.refs {
            self.panel.render(f, refs_area, &ctx, true);
        }
        list_state.workspace.areas = Some(areas);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let areas = main_areas(&self.ctx, area, Some(&self.panel), preview_on(&self.ctx));
        let ctx = self.ctx.clone();
        let list_state = self.as_mut_list_state();
        if let Some(panel_area) = areas.panel {
            list_state.workspace.clear_rows_under(&ctx, panel_area);
        }
        let list = areas.list;
        let height = if list.height >= 2 {
            list.height - 2
        } else {
            list.height
        };
        list_state.update_height(height as usize);
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

impl<'a> RefsView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        let mut list_state = self.commit_list_state.take().unwrap();
        // The tree goes along with the list (sidebar of the next view).
        list_state.workspace.refs = Some(std::mem::take(&mut self.panel));
        list_state
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

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    fn update_commit_list_selected(&mut self) {
        if let Some(selected) = self.panel.state.selected_ref_name() {
            self.as_mut_list_state().select_ref(&selected)
        }
    }

    fn copy_ref_name(&self) {
        if let Some(selected) = self.panel.state.selected_branch() {
            self.copy_to_clipboard("Branch Name".into(), selected);
        } else if let Some(selected) = self.panel.state.selected_tag() {
            self.copy_to_clipboard("Tag Name".into(), selected);
        }
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&mut self) {
        self.panel.worktrees = actions::list_worktrees(std::path::Path::new("."));
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let (tree_selected, tree_opened) = self.panel.state.current_tree_status();
        let refs_context = RefsRefreshViewContext {
            selected: tree_selected,
            opened: tree_opened,
        };
        let context = RefreshViewContext::Refs {
            list_context,
            refs_context,
        };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn reset_refs_with(&mut self, refs_context: RefsRefreshViewContext) {
        self.panel
            .state
            .reset_tree_status(refs_context.selected, refs_context.opened);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        self.panel.state.handle_click(col, row);
        if self.panel.state.selected_is_add_remote_item() {
            self.tx.send(AppEvent::OpenDialog(DialogKind::AddRemote));
        } else if self.panel.state.selected_is_add_worktree_item() {
            self.tx.send(AppEvent::OpenDialog(DialogKind::AddWorktree));
        } else if self.panel.state.selected_is_node() {
            self.update_commit_list_selected();
        } else if let Some(wt_path) = self.panel.state.selected_worktree_path() {
            let display_name = self
                .panel
                .worktrees
                .iter()
                .find(|wt| wt.path == wt_path)
                .and_then(|wt| wt.branch.as_deref())
                .and_then(|b| b.strip_prefix("refs/heads/"))
                .unwrap_or(&wt_path)
                .to_string();
            self.tx
                .send(AppEvent::OpenDialog(DialogKind::ConfirmSwitchWorktree {
                    path: wt_path,
                    display_name,
                }));
        } else {
            // A branch or tag: select it and bring its commit up in the
            // list (Enter opens its detail).
            self.update_commit_list_selected();
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if self.panel.state.handle_mouse_move(col, row) {
            self.update_commit_list_selected();
        }
    }
}

/// The refs tree and its state: owned by the Refs view while it has focus,
/// otherwise carried in the commit list's workspace as the sidebar.
#[derive(Debug, Default)]
pub(crate) struct RefsPanel {
    pub refs: Vec<Ref>,
    pub worktrees: Vec<actions::WorktreeInfo>,
    pub state: RefListState,
}

impl RefsPanel {
    pub fn load(refs: Vec<Ref>) -> Self {
        RefsPanel {
            refs,
            worktrees: actions::list_worktrees(std::path::Path::new(".")),
            state: RefListState::new(),
        }
    }

    /// `[commit list column, refs]`, on the configured side.
    ///
    /// Adaptive width: each ref takes tree-depth indent + open/close symbol
    /// (2 cells) + its last path segment, plus 4 cells of border and padding;
    /// clamped to `[MIN_WIDTH, ui.refs.width]` and to what leaves the commit
    /// list `LIST_MIN_WIDTH` columns.
    pub fn split(&self, ctx: &AppContext, area: Rect) -> [Rect; 2] {
        const MIN_WIDTH: u16 = 16;
        const LIST_MIN_WIDTH: u16 = 60;
        const CHROME: u16 = 4; // border + padding
        let natural = self
            .refs
            .iter()
            .map(|r| {
                let name = match r {
                    Ref::Branch { name, .. }
                    | Ref::RemoteBranch { name, .. }
                    | Ref::Tag { name, .. }
                    | Ref::Stash { name, .. } => name.as_str(),
                };
                let parts = name.split('/').count();
                let last = name.rsplit('/').next().unwrap_or(name);
                let depth = parts.saturating_sub(1) as u16;
                depth * 2 + 2 + console::measure_text_width(last) as u16
            })
            .max()
            .unwrap_or(0)
            .saturating_add(CHROME);
        let cap = ctx.ui_config.refs.width;
        let area_cap = area.width.saturating_sub(LIST_MIN_WIDTH);
        let refs_width = natural.clamp(MIN_WIDTH, cap).min(area_cap);
        match ctx.ui_config.refs.position {
            RefsPosition::Right => {
                Layout::horizontal([Constraint::Min(0), Constraint::Length(refs_width)]).areas(area)
            }
            RefsPosition::Left => {
                let [refs_area, list_area] =
                    Layout::horizontal([Constraint::Length(refs_width), Constraint::Min(0)])
                        .areas(area);
                [list_area, refs_area]
            }
        }
    }

    /// "Refs" header (accented while focused) and the tree.
    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Rc<AppContext>, focused: bool) {
        use ratatui::{
            style::{Modifier, Style},
            text::{Line, Span},
            widgets::Paragraph,
        };
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        let theme = &ctx.color_theme;
        let title_fg = if focused {
            theme.list_head_fg
        } else {
            theme.list_ref_paren_fg
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Refs",
                Style::default().fg(title_fg).add_modifier(Modifier::BOLD),
            ))),
            Rect::new(header_area.x, header_area.y, header_area.width, 1),
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "╌".repeat(header_area.width as usize),
                Style::default().fg(theme.divider_fg),
            ))),
            Rect::new(header_area.x, header_area.y + 1, header_area.width, 1),
        );
        let ref_list = RefList::new(&self.refs, &self.worktrees, ctx.clone()).with_focus(focused);
        f.render_stateful_widget(ref_list, list_area, &mut self.state);
    }
}
