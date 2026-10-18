//! Searches the last complete history during resize, then projects only the
//! displayed matches through a bounded reflow viewport. Neither searching nor
//! navigation waits for the full-history worker.

use super::*;
use alacritty_terminal::index::Point as AlacPoint;
use alacritty_terminal::term::{ReflowListener, ReflowSearchView, Term};

type Source = Arc<Term<ReflowListener>>;
type SourceSlot = Arc<std::sync::Mutex<Option<Source>>>;

#[derive(Default)]
pub(super) struct ReflowSearch {
    pub(super) source: SourceSlot,
    selected: Option<Range>,
    anchor: Option<Range>,
    ensure_anchor_visible: bool,
    projected: Arc<SearchRanges>,
}

impl ReflowSearch {
    pub(super) fn source(&self) -> Option<Arc<Term<ReflowListener>>> {
        self.source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Terminal {
    pub(super) fn reflow_search_selection(&self) -> Option<Range> {
        self.reflow_search.selected
    }
    pub(super) fn clear_reflow_search_selection(&mut self) {
        self.reflow_search.selected = None;
        self.content_dirty = true;
    }
    pub(super) fn reflow_search_scrolled(&mut self) {
        self.reflow_search.ensure_anchor_visible = false;
    }
    pub fn renderable_search_matches(&self) -> Arc<SearchRanges> {
        if self.reflow_search.anchor.is_some() && !self.matches.is_empty() {
            self.reflow_search.projected.clone()
        } else if self.reflow_search.source().is_some() {
            Arc::default()
        } else {
            self.matches.clone()
        }
    }

    pub(super) fn activate_reflow_search(&mut self, range: Range) -> bool {
        let Some(source) = self.reflow_search.source() else {
            return false;
        };
        let mut term = self.term.lock_unfair();
        let Some(current) = term.reflow_search_source() else {
            return false;
        };
        if !Arc::ptr_eq(&source, &current) {
            return false;
        }
        let offset = term.reflow_search_offset().unwrap_or_default();
        let target = range.start().line.saturating_neg().max(0) as usize;
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(
            target as i32 - offset as i32,
        ));
        self.reflow_search.selected = Some(range);
        self.reflow_search.anchor = Some(range);
        self.reflow_search.ensure_anchor_visible = true;
        self.content_dirty = true;
        true
    }

    pub(super) fn render_reflow_search(&mut self, term: &AlacrittyTerm) -> bool {
        let Some(anchor) = self
            .reflow_search
            .anchor
            .filter(|_| !self.matches.is_empty())
        else {
            return false;
        };
        let focus = self.reflow_search.ensure_anchor_visible.then(|| {
            AlacPoint::new(
                alacritty_terminal::index::Line(anchor.start().line),
                alacritty_terminal::index::Column(anchor.start().column),
            )
        });
        let Some(ReflowSearchView {
            term: view,
            offset,
            total_lines: total,
            cells: mapping,
        }) = term.reflow_search_view(focus)
        else {
            return false;
        };
        self.last_content = make_content(&view, &mut self.last_content);
        self.last_content.display_offset = offset;
        self.last_content.total_lines = total;
        self.last_content.scrolled_to_top = offset + view.screen_lines() == total;
        self.last_content.scrolled_to_bottom = offset == 0;
        // Content coordinates follow the source scrollbar, just like ordinary
        // pending-reflow browsing. The cell map preserves wide-cell geometry.
        for cell in &mut self.last_content.cells {
            cell.point.line -= offset as i32;
        }
        let mut ranges: Vec<Range> = Vec::new();
        let mut selection: Option<SelectionRange> = None;
        for (target, original) in mapping {
            let source = Point::new(original.line.0, original.column.0);
            let point = Point::new(target.line.0 - offset as i32, target.column.0);
            if self
                .reflow_search
                .selected
                .is_some_and(|selected| selected.contains(source))
            {
                match &mut selection {
                    Some(selection) => selection.end = point,
                    None => {
                        selection = Some(SelectionRange {
                            start: point,
                            end: point,
                            is_block: false,
                        })
                    }
                }
            }
            if self.matches.contains_point(source) {
                if let Some(last) = ranges.last_mut()
                    && last.end().line == point.line
                    && last.end().column + 1 == point.column
                {
                    *last = Range::new(last.start(), point);
                } else {
                    ranges.push(Range::new(point, point));
                }
            }
        }
        let mut projected = SearchRanges::default();
        projected.extend_older(ranges.into_iter().rev());
        self.reflow_search.projected = Arc::new(projected);
        self.last_content.selection = selection;
        true
    }
}

#[cfg(test)]
#[path = "tests/search_reflow.rs"]
mod tests;
