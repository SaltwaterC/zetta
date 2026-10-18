//! Pane search overlay presentation. Search jobs and navigation live in the
//! terminal crate's shared session; this module only draws the query and status.

use super::*;

impl TerminalView {
    pub(super) fn render_search_overlay(&self, query: &str, theme: &Theme) -> AnyElement {
        let status = self.search_status.label();
        let cursor = self.search_cursor.min(query.len());
        let (query_before, query_after) = query.split_at(cursor);
        let query_before = query_before.to_owned();
        let query_after = query_after.to_owned();
        let query_selected = self.search_select_all;
        let search_focus_handle = self.search_focus_handle.clone();

        div()
            .absolute()
            .top_2()
            .left_2()
            .right_2()
            .flex()
            .justify_end()
            .child(
                div()
                    .id("terminal-search")
                    .track_focus(&self.search_focus_handle)
                    .w_full()
                    .max_w(px(440.0))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().elevated_surface_background.alpha(1.0))
                    .shadow_sm()
                    .text_sm()
                    .text_color(theme.colors().text)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        search_focus_handle.focus(window, cx);
                        cx.stop_propagation();
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .flex_basis(px(120.))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .debug_selector(|| format!("terminal-search-query:{query}"))
                                    .when(query_selected, |input| {
                                        input.bg(theme.colors().element_selection_background)
                                    })
                                    .child(div().whitespace_nowrap().child(query_before))
                                    .when(!query_selected, |input| {
                                        input.child(
                                            div()
                                                .flex_none()
                                                .w(px(1.0))
                                                .h(px(16.0))
                                                .bg(theme.colors().text_accent),
                                        )
                                    })
                                    .child(div().whitespace_nowrap().child(query_after)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .max_w_full()
                                    .text_color(theme.colors().text_muted)
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors().text_muted)
                            .child("Enter next  Shift+Enter previous  Esc close"),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "tests/search_overlay.rs"]
mod tests;
