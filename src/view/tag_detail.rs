use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    view::workspace::{layout_with_panel, render_list_and_sidebar},
    widget::commit_list::CommitListState,
    widget::tag_detail::{TagDetail, TagDetailState, TagMetadata},
};

#[derive(Debug)]
pub struct TagDetailView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    tag_detail_state: TagDetailState,
    metadata: TagMetadata,
    ctx: Rc<AppContext>,
    tx: Sender,
    detail_area: Option<Rect>,
}

impl<'a> TagDetailView<'a> {
    pub fn new(
        _tag_name: String,
        metadata: TagMetadata,
        commit_list_state: Option<CommitListState<'a>>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            tag_detail_state: TagDetailState::default(),
            metadata,
            ctx,
            tx,
            detail_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        match event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::NavigateDown | UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_down();
                }
            }
            UserEvent::NavigateUp | UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_up();
                }
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_page_down();
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_page_up();
                }
            }
            UserEvent::HalfPageDown => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_half_page_down();
                }
            }
            UserEvent::HalfPageUp => {
                for _ in 0..count {
                    self.tag_detail_state.scroll_half_page_up();
                }
            }
            UserEvent::GoToTop => {
                self.tag_detail_state.select_first();
            }
            UserEvent::GoToBottom => {
                self.tag_detail_state.select_last();
            }
            UserEvent::PushTag | UserEvent::Push => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::PushTag {
                    tag: self.metadata.tag_name.clone(),
                }));
            }
            UserEvent::DeleteTag | UserEvent::DeleteBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteTag {
                    tag: self.metadata.tag_name.clone(),
                }));
            }
            UserEvent::CopyTagName => {
                self.tx.send(AppEvent::CopyToClipboard {
                    name: "Tag Name".into(),
                    value: self.metadata.tag_name.clone(),
                });
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // The List view's layout, with the tag in the panel.
        let areas = layout_with_panel(&self.ctx, area, self.commit_list_state.as_ref());
        let detail_area = match self.commit_list_state {
            Some(ref mut list_state) => {
                render_list_and_sidebar(f, &self.ctx, areas, list_state);
                areas.panel.unwrap_or(area)
            }
            None => Rect {
                height: area.height.min(10),
                ..area
            },
        };

        let tag_detail = TagDetail::new(&self.metadata, self.ctx.clone());
        f.render_stateful_widget(tag_detail, detail_area, &mut self.tag_detail_state);
        self.detail_area = Some(detail_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let areas = layout_with_panel(&self.ctx, area, self.commit_list_state.as_ref());
        if let Some(ref mut list_state) = self.commit_list_state {
            if let Some(panel) = areas.panel {
                list_state.workspace.clear_rows_under(&self.ctx, panel);
            }
            list_state.update_height(areas.list.height as usize);
        }
    }

    pub fn list_state(&self) -> Option<&CommitListState<'a>> {
        self.commit_list_state.as_ref()
    }

    pub fn list_state_mut(&mut self) -> Option<&mut CommitListState<'a>> {
        self.commit_list_state.as_mut()
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y {
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                if let Some(action_idx) = self.action_index_at_row(action_bar_row) {
                    self.execute_action(action_idx);
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y {
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                self.tag_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            } else {
                self.tag_detail_state.hovered_action = None;
            }
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        self.tx.send(AppEvent::OpenTagDetail {
            tag_name: self.metadata.tag_name.clone(),
        });
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.ensure_visible_graph_uploaded();
            list_state.ensure_visible_avatars_uploaded(
                &mut self.ctx.avatar_manager.lock().unwrap(),
                self.ctx.color_theme.bg,
                self.ctx.color_theme.list_selected_bg,
            );
        }
    }

    pub fn clear_graph_images(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.clear_graph_images();
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.drain_pending_graph_uploads()
        } else {
            Vec::new()
        }
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        if let Some(ref list_state) = self.commit_list_state {
            list_state.graph_image_ids_sorted()
        } else {
            Vec::new()
        }
    }

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        if action_bar_row < crate::widget::tag_detail::TAG_ACTIONS.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        let name = self.metadata.tag_name.clone();
        match action_idx {
            0 => self
                .tx
                .send(AppEvent::OpenDialog(DialogKind::PushTag { tag: name })),
            1 => self
                .tx
                .send(AppEvent::OpenDialog(DialogKind::DeleteTag { tag: name })),
            2 => self.tx.send(AppEvent::CopyToClipboard {
                name: "Tag Name".into(),
                value: name,
            }),
            _ => {}
        }
    }
}
