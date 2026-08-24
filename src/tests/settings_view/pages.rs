use super::*;
use gpui::TestAppContext;

struct ProfileFieldsGridHarness;

impl Render for ProfileFieldsGridHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let fields = (0..4).map(|index| {
            div()
                .min_w(px(180.))
                .h_9()
                .debug_selector(move || format!("profile-field-{index}"))
                .into_any_element()
        });
        div()
            .w(px(488.))
            .p_3()
            .debug_selector(|| "profile-card".to_owned())
            .child(profile_fields_grid(fields))
    }
}

#[gpui::test]
fn profile_fields_stay_inside_the_card_at_the_minimum_dialog_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ProfileFieldsGridHarness);
    cx.run_until_parked();

    let card = cx
        .debug_bounds("profile-card")
        .expect("profile card bounds");
    for (index, name) in [
        "profile-field-0",
        "profile-field-1",
        "profile-field-2",
        "profile-field-3",
    ]
    .into_iter()
    .enumerate()
    {
        let field = cx.debug_bounds(name).expect("profile field bounds");
        assert!(
            field.left() >= card.left() && field.right() <= card.right(),
            "profile field {index} must not overflow the card: {field:?} outside {card:?}"
        );
        assert!(
            field.size.width >= px(180.),
            "profile field {index} must remain usable instead of collapsing"
        );
    }
}
