use super::*;
use gpui::{TestAppContext, size};

#[test]
fn script_open_keyboard_defaults_to_cancel() {
    assert_eq!(script_open_action("enter", false), ScriptOpenAction::Cancel);
    assert_eq!(script_open_action("enter", true), ScriptOpenAction::Run);
    assert_eq!(script_open_action("escape", true), ScriptOpenAction::Cancel);
    assert_eq!(script_open_action("tab", false), ScriptOpenAction::Choose);
    assert_eq!(script_open_action("right", true), ScriptOpenAction::Choose);
    assert_eq!(script_open_action("space", false), ScriptOpenAction::Ignore);
}

#[test]
fn script_open_requests_are_answered_one_at_a_time() {
    let mut queue = ScriptOpenQueue::default();
    assert!(!queue.is_open());
    queue.push(Vec::new());
    assert!(!queue.is_open(), "an open with no scripts prompts nothing");

    let first = vec![PathBuf::from("/tmp/a.command"), PathBuf::from("/tmp/b.sh")];
    let second = vec![PathBuf::from("/tmp/c.sh")];
    queue.push(first.clone());
    queue.run_selected = true;
    queue.push(second.clone());

    assert_eq!(queue.showing(), Some(first.as_slice()));
    assert_eq!(queue.answer(), Some(first));
    assert!(
        !queue.run_selected,
        "a script that arrived later must not inherit the earlier choice"
    );
    assert_eq!(queue.showing(), Some(second.as_slice()));
    assert_eq!(queue.answer(), Some(second));
    assert!(!queue.is_open());
    assert_eq!(queue.answer(), None);
}

#[test]
fn clearing_drops_every_waiting_script() {
    let mut queue = ScriptOpenQueue::default();
    queue.push(vec![PathBuf::from("/tmp/a.command")]);
    queue.push(vec![PathBuf::from("/tmp/b.command")]);
    queue.run_selected = true;
    queue.clear();
    assert!(!queue.is_open());
    assert!(!queue.run_selected);
}

struct DialogHarness {
    scripts: Vec<PathBuf>,
    focus: gpui::FocusHandle,
}

impl Render for DialogHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(script_open_dialog(
            &self.scripts,
            false,
            &self.focus,
            cx.theme().colors(),
            |_, _, _| {},
            |_, _, _| {},
        ))
    }
}

#[gpui::test]
fn script_open_dialog_keeps_actions_visible_with_many_long_paths(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (_view, cx) = cx.add_window_view(|_, cx| DialogHarness {
        scripts: (0..60)
            .map(|index| {
                PathBuf::from(format!(
                    "/{}script-{index}.command",
                    "long-download-folder/".repeat(12)
                ))
            })
            .collect(),
        focus: cx.focus_handle(),
    });
    cx.simulate_resize(size(px(520.), px(360.)));
    let panel = cx.debug_bounds("script-open-panel").unwrap();
    let actions = cx.debug_bounds("script-open-actions").unwrap();
    let paths = cx.debug_bounds("script-open-paths").unwrap();
    assert!(paths.size.height > px(0.));
    assert!(paths.bottom() <= actions.top());
    assert!(actions.bottom() <= panel.bottom());
    assert!(actions.right() <= panel.right());
    assert!(panel.bottom() <= px(360.));
}
