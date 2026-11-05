//! Panels kept on screen around the commit list: the refs sidebar and the
//! preview panel. They ride in the commit list state that the List, Refs
//! and Detail views hand around, so switching views keeps the layout.

use std::rc::Rc;

use ratatui::{
    layout::{Position, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::Sender,
    git::CommitHash,
    view::{
        detail::{clear_rows_under, split_list_detail, DetailPreview},
        refs::RefsPanel,
    },
    widget::commit_list::{CommitList, CommitListState},
};

/// Where the last render put the refs sidebar, commit list and panel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MainAreas {
    pub refs: Option<Rect>,
    pub list: Rect,
    pub panel: Option<Rect>,
}

pub(crate) fn preview_on(ctx: &AppContext) -> bool {
    ctx.ui_config.detail.preview
}

/// Refs sidebar beside (if `refs` is given), commit list on top of the
/// detail panel (if `panel`).
pub(crate) fn main_areas(
    ctx: &AppContext,
    area: Rect,
    refs: Option<&RefsPanel>,
    panel: bool,
) -> MainAreas {
    let (column, refs) = match refs {
        Some(r) => {
            let [list_column, refs_area] = r.split(ctx, area);
            (list_column, Some(refs_area))
        }
        None => (area, None),
    };
    let (list, panel) = if panel {
        let [list, panel] = split_list_detail(ctx, column);
        (list, Some(panel))
    } else {
        (column, None)
    };
    MainAreas { refs, list, panel }
}

#[derive(Debug, Default)]
pub struct Workspace<'a> {
    preview: Option<Box<DetailPreview<'a>>>,
    /// Recently shown previews, oldest first.
    preview_cache: Vec<DetailPreview<'a>>,
    pub(crate) refs: Option<RefsPanel>,
    cleared_detail_area: Option<Rect>,
    /// Areas of the last render, for routing mouse clicks to a window.
    pub(crate) areas: Option<MainAreas>,
}

impl<'a> Workspace<'a> {
    pub fn preview_hash(&self) -> Option<&CommitHash> {
        self.preview.as_ref().map(|p| &p.commit.commit_hash)
    }

    /// Show `preview`, keeping the replaced one so going back to it skips git.
    pub fn set_preview(&mut self, preview: Option<DetailPreview<'a>>) {
        const KEEP: usize = 32;
        if let Some(old) = std::mem::replace(&mut self.preview, preview.map(Box::new)) {
            self.preview_cache.push(*old);
            if self.preview_cache.len() > KEEP {
                self.preview_cache.remove(0);
            }
        }
    }

    /// Show the kept preview of `hash`, if any. Returns whether it did.
    pub fn restore_preview(&mut self, hash: &CommitHash) -> bool {
        let Some(i) = self
            .preview_cache
            .iter()
            .position(|p| &p.commit.commit_hash == hash)
        else {
            return false;
        };
        let preview = self.preview_cache.remove(i);
        self.set_preview(Some(preview));
        true
    }

    pub fn render_preview(&mut self, f: &mut Frame, area: Rect, ctx: &Rc<AppContext>, tx: &Sender) {
        if let Some(preview) = self.preview.as_mut() {
            preview.render(f, area, ctx, tx);
        }
    }

    /// Wheel over the preview panel or refs sidebar scrolls it; false elsewhere.
    pub fn handle_wheel(&mut self, col: u16, row: u16, down: bool) -> bool {
        if self
            .preview
            .as_mut()
            .is_some_and(|p| p.handle_wheel(col, row, down))
        {
            return true;
        }
        let over_refs = self
            .areas
            .and_then(|a| a.refs)
            .is_some_and(|a| a.contains(Position::new(col, row)));
        match self.refs.as_mut() {
            Some(refs) if over_refs => {
                if down {
                    refs.state.scroll_down(3);
                } else {
                    refs.state.scroll_up(3);
                }
                true
            }
            _ => false,
        }
    }

    /// Delete Kitty graph placements on the rows the panel now covers.
    pub fn clear_rows_under(&mut self, ctx: &AppContext, panel: Rect) {
        clear_rows_under(ctx, panel, &mut self.cleared_detail_area);
    }
}

/// The List view's layout for a view with its own panel content (branch /
/// tag detail). Without a commit list the panel takes the whole area.
pub(crate) fn layout_with_panel(
    ctx: &AppContext,
    area: Rect,
    list_state: Option<&CommitListState<'_>>,
) -> MainAreas {
    match list_state {
        Some(list_state) => {
            let refs = if ctx.ui_config.refs.always_open {
                list_state.workspace.refs.as_ref()
            } else {
                None
            };
            main_areas(ctx, area, refs, true)
        }
        None => MainAreas {
            refs: None,
            list: Rect::default(),
            panel: Some(area),
        },
    }
}

/// Draw the unfocused list and the refs sidebar; the caller draws the panel.
pub(crate) fn render_list_and_sidebar(
    f: &mut Frame,
    ctx: &Rc<AppContext>,
    areas: MainAreas,
    list_state: &mut CommitListState<'_>,
) {
    let commit_list = CommitList::new(ctx.clone()).with_focus(Some(false));
    f.render_stateful_widget(commit_list, areas.list, list_state);
    if let (Some(refs_area), Some(refs)) = (areas.refs, list_state.workspace.refs.as_mut()) {
        refs.render(f, refs_area, ctx, false);
    }
    list_state.workspace.areas = Some(areas);
}
