use super::*;
use gpui::{TestAppContext, px};
use theme::ThemeColors;

/// Lays two [`field_query_run`]s side by side, each sized to its own content,
/// so a test can compare what the two of them emit.
///
/// `h_flex` rather than a plain column on purpose: a block child would stretch
/// to the harness width and every run would measure the same.
struct QueryRunHarness {
    left: TextField,
    right: TextField,
    placeholder: Option<&'static str>,
}

impl Render for QueryRunHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ThemeColors::dark();
        h_flex()
            .w(px(800.))
            .child(
                div()
                    .flex_none()
                    .debug_selector(|| "left".to_owned())
                    .child(field_query_run(&self.left, self.placeholder, &colors)),
            )
            .child(
                div()
                    .flex_none()
                    .debug_selector(|| "right".to_owned())
                    .child(field_query_run(&self.right, self.placeholder, &colors)),
            )
            .child(
                div()
                    .flex_none()
                    .debug_selector(|| "boxed".to_owned())
                    .child(field_box("boxed-field", true, &colors).child("x")),
            )
    }
}

/// The two runs' widths, in that order.
fn run_widths(
    left: TextField,
    right: TextField,
    placeholder: Option<&'static str>,
    cx: &mut TestAppContext,
) -> (Pixels, Pixels) {
    let (_view, cx) = cx.add_window_view(|_, _| QueryRunHarness {
        left,
        right,
        placeholder,
    });
    cx.run_until_parked();
    (
        cx.debug_bounds("left").expect("left run bounds").size.width,
        cx.debug_bounds("right")
            .expect("right run bounds")
            .size
            .width,
    )
}

/// A selected field shows the selection background and no caret: the selection
/// is what marks the position, and a caret inside it reads as a second cursor.
/// The caret is one pixel, so dropping it is exactly one pixel narrower.
#[gpui::test]
fn a_selected_field_renders_without_the_caret(cx: &mut TestAppContext) {
    let unselected = TextField::new("abcdef");
    let mut selected = TextField::new("abcdef");
    selected.select_all = true;

    let (plain, selected) = run_widths(unselected, selected, None, cx);
    assert_eq!(
        plain - selected,
        px(1.),
        "the only difference between the two is the one-pixel caret"
    );
}

/// The placeholder is rendered only while the field is empty, so a field with
/// text does not show a suggestion behind what was typed.
#[gpui::test]
fn the_placeholder_shows_only_while_the_field_is_empty(cx: &mut TestAppContext) {
    let (empty, typed) = run_widths(
        TextField::default(),
        TextField::new("a"),
        Some("Type a command"),
        cx,
    );
    assert!(
        empty > typed,
        "an empty field shows the placeholder and a typed one does not: {empty:?} vs {typed:?}"
    );

    let (no_placeholder, _) = run_widths(TextField::default(), TextField::default(), None, cx);
    assert!(
        no_placeholder < empty,
        "a field with nothing to suggest emits no element for it"
    );
}

/// The cursor position is what the run splits on, so moving it must not change
/// how much is drawn — only where the caret lands within it.
#[gpui::test]
fn moving_the_cursor_does_not_change_what_the_run_draws(cx: &mut TestAppContext) {
    let at_end = {
        let mut field = TextField::new("abcdef");
        field.move_to_end();
        field
    };
    let in_middle = {
        let mut field = TextField::new("abcdef");
        field.cursor = 3;
        field
    };
    let (end, middle) = run_widths(at_end, in_middle, None, cx);
    assert_eq!(end, middle);
}

/// Slicing a `String` anywhere but a character boundary panics, and the run is
/// where the two halves are first taken — so every boundary of a multi-byte
/// string has to render.
#[gpui::test]
fn every_cursor_position_in_multi_byte_text_renders(cx: &mut TestAppContext) {
    let text = "héllo wörld";
    let mut cursor = text.len();
    while cursor > 0 {
        let mut field = TextField::new(text);
        field.cursor = cursor;
        let (width, _) = run_widths(field, TextField::default(), None, cx);
        assert!(width > px(0.), "cursor at {cursor} rendered nothing");
        cursor = crate::text_edit::previous_char_boundary(text, cursor);
    }
}

/// A boxed field is one row high whatever it holds, so a form's rows line up
/// with the dropdowns and buttons beside them.
#[gpui::test]
fn a_boxed_field_is_one_row_high(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| QueryRunHarness {
        left: TextField::default(),
        right: TextField::default(),
        placeholder: None,
    });
    cx.run_until_parked();
    let boxed = cx.debug_bounds("boxed").expect("boxed field bounds");
    assert_eq!(boxed.size.height, px(36.), "h_9 plus its one-pixel borders");
}
