use super::*;
use gpui::{TestAppContext, size};

struct ReviewHarness {
    prompt: ProjectTrustPrompt,
}

impl Render for ReviewHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(project_trust_dialog(
            &self.prompt,
            cx.theme().colors(),
            |_, _, _| {},
            |_, _, _| {},
        ))
    }
}

#[gpui::test]
fn project_trust_review_keeps_actions_visible_with_long_content(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (_view, cx) = cx.add_window_view(|_, cx| ReviewHarness {
        prompt: ProjectTrustPrompt {
            root: PathBuf::from(format!("/{}", "long-project-path/".repeat(30))),
            fingerprint: "reviewed".to_owned(),
            review: "{\n  \"commands\": \"review this command\"\n}\n"
                .repeat(100)
                .into(),
            focus: cx.focus_handle(),
            trust_selected: false,
            saving: false,
        },
    });
    cx.simulate_resize(size(px(520.), px(360.)));
    let panel = cx.debug_bounds("project-trust-panel").unwrap();
    let actions = cx.debug_bounds("project-trust-actions").unwrap();
    let review = cx.debug_bounds("project-trust-review").unwrap();
    assert!(review.size.height > px(0.));
    assert!(review.bottom() <= actions.top());
    assert!(actions.bottom() <= panel.bottom());
    assert!(actions.right() <= panel.right());
    assert!(panel.bottom() <= px(360.));
}

#[test]
fn project_trust_keyboard_defaults_to_continuing_without_approval() {
    assert_eq!(
        review_action("enter", false, false),
        ReviewAction::ContinueWithout
    );
    assert_eq!(review_action("enter", true, false), ReviewAction::Approve);
    assert_eq!(
        review_action("escape", true, false),
        ReviewAction::ContinueWithout
    );
    assert_eq!(review_action("tab", false, false), ReviewAction::Choose);
    assert_eq!(review_action("enter", true, true), ReviewAction::Ignore);
    assert_eq!(review_action("escape", false, true), ReviewAction::Ignore);
}
