//! The preview panel under the commit list. It rides in the commit list
//! state that the List and Detail views hand around, so switching views
//! keeps the layout.

use std::rc::Rc;

use ratatui::{layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::Sender,
    git::CommitHash,
    view::detail::{clear_rows_under, split_list_detail, DetailPreview},
};

/// Where the commit list and the detail panel go.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MainAreas {
    pub list: Rect,
    pub panel: Option<Rect>,
}

pub(crate) fn preview_on(ctx: &AppContext) -> bool {
    ctx.ui_config.detail.preview
}

/// Commit list on top of the detail panel (if `panel`).
pub(crate) fn main_areas(ctx: &AppContext, area: Rect, panel: bool) -> MainAreas {
    let (list, panel) = if panel {
        let [list, panel] = split_list_detail(ctx, area);
        (list, Some(panel))
    } else {
        (area, None)
    };
    MainAreas { list, panel }
}

#[derive(Debug, Default)]
pub struct Workspace<'a> {
    preview: Option<Box<DetailPreview<'a>>>,
    /// Recently shown previews, oldest first.
    preview_cache: Vec<DetailPreview<'a>>,
    cleared_detail_area: Option<Rect>,
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

    /// Wheel over the preview panel scrolls it; false elsewhere.
    pub fn handle_wheel(&mut self, col: u16, row: u16, down: bool) -> bool {
        self.preview
            .as_mut()
            .is_some_and(|p| p.handle_wheel(col, row, down))
    }

    /// Delete Kitty graph placements on the rows the panel now covers.
    pub fn clear_rows_under(&mut self, ctx: &AppContext, panel: Rect) {
        clear_rows_under(ctx, panel, &mut self.cleared_detail_area);
    }
}
