use super::*;

#[test]
fn pane_resize_mode_uses_a_twenty_pixel_mouse_gutter() {
    assert_eq!(PANE_RESIZE_GUTTER_SIZE, px(20.));
}

#[test]
fn pane_resize_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_resize_menu_entry_available(0));
    assert!(!pane_resize_menu_entry_available(1));
    assert!(pane_resize_menu_entry_available(2));
    assert!(pane_resize_menu_entry_available(3));
}

#[test]
fn pane_move_menu_entry_requires_at_least_two_panes() {
    assert!(!pane_move_menu_entry_available(0));
    assert!(!pane_move_menu_entry_available(1));
    assert!(pane_move_menu_entry_available(2));
    assert!(pane_move_menu_entry_available(3));
}

#[test]
fn stacked_rows_match_the_terminal_background_layers() {
    let mut colors = ThemeColors::light();
    colors.tab_active_background = gpui::rgb(0x112233).into();
    colors.editor_background = gpui::rgb(0x445566).into();
    colors.terminal_background = gpui::rgb(0xaabbcc).into();

    let mut backdrop = stacked_rows_backdrop(colors.editor_background);
    let mut rows = stacked_rows_container(colors.terminal_background);
    let backdrop_background = backdrop
        .style()
        .background
        .as_ref()
        .and_then(gpui::Fill::color);
    let background = rows.style().background.as_ref().and_then(gpui::Fill::color);

    assert_eq!(backdrop_background, Some(colors.editor_background.into()));
    assert_eq!(background, Some(colors.terminal_background.into()));
    assert_ne!(background, Some(colors.tab_active_background.into()));
}

#[test]
fn inactive_opacity_is_shared_by_stacked_rows_and_pane_content() {
    let mut active_surface = with_inactive_pane_opacity(div(), true, 0.65);
    let mut inactive_surface = with_inactive_pane_opacity(div(), false, 0.65);

    assert_eq!(active_surface.style().opacity, None);
    assert_eq!(inactive_surface.style().opacity, Some(0.65));
}

struct TerminalPlaceholderTestView {
    focus_handle: gpui::FocusHandle,
    saw_new_tab: bool,
}

impl Render for TerminalPlaceholderTestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Zetta")
            .size_full()
            .on_action(cx.listener(|this, _: &NewTab, _, _| {
                this.saw_new_tab = true;
            }))
            .child(terminal_focus_placeholder(
                &self.focus_handle,
                div().size_full(),
            ))
    }
}

#[gpui::test]
fn no_view_placeholder_preserves_terminal_action_routing(cx: &mut gpui::TestAppContext) {
    let window = cx.open_window(size(px(100.), px(100.)), |_, cx| {
        TerminalPlaceholderTestView {
            focus_handle: cx.focus_handle(),
            saw_new_tab: false,
        }
    });
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-t", NewTab, Some("Zetta > Terminal"))]);
    });
    cx.run_until_parked();

    window
        .update(cx, |view, window, cx| {
            view.focus_handle.focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    window
        .update(cx, |_, window, _| {
            assert_eq!(
                window.context_stack(),
                vec![
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("Terminal").unwrap(),
                ]
            );
        })
        .unwrap();

    cx.simulate_keystrokes(*window, "ctrl-t");
    window
        .update(cx, |view, _, _| assert!(view.saw_new_tab))
        .unwrap();
}

#[test]
fn pane_window_edges_follow_split_direction() {
    let edges = PaneWindowEdges::all();
    assert!(!edges.with_bottom(false).bottom);

    let top = edges.first(SplitAxis::Horizontal);
    let bottom = edges.second(SplitAxis::Horizontal);
    assert!(top.left && top.right && !top.bottom);
    assert!(bottom.left && bottom.right && bottom.bottom);

    let left = edges.first(SplitAxis::Vertical);
    let right = edges.second(SplitAxis::Vertical);
    assert!(left.left && left.bottom && !left.right);
    assert!(!right.left && right.bottom && right.right);
}
