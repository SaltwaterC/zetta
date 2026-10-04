use super::*;

fn window(cx: &mut gpui::TestAppContext) -> (Entity<Zetta>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(
            theme::LoadThemes::All(Box::new(crate::zetta_assets::ZettaAssets)),
            cx,
        );
        let registry = ThemeRegistry::global(cx);
        theme_settings::load_bundled_themes(&registry);
        theme::GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    cx.executor().allow_parking();
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear(); // No shell or daemon in this window.
        Zetta::new(
            config,
            None,
            crate::ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.toggle_command_palette(&ToggleCommandPalette, window, cx);
    });
    cx.run_until_parked();
    (zetta, cx)
}

fn key(key: &str, text: Option<&str>) -> KeyDownEvent {
    let mut keystroke = gpui::Keystroke::parse(key).unwrap();
    keystroke.key_char = text.map(str::to_owned);
    KeyDownEvent {
        keystroke,
        is_held: false,
        prefer_character_input: false,
    }
}

fn send(zetta: &Entity<Zetta>, cx: &mut gpui::VisualTestContext, key: &str, text: Option<&str>) {
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.command_palette_key_down(&self::key(key, text), window, cx);
    });
}

#[gpui::test]
fn slow_overlay_paste_keeps_typing_and_consecutive_pastes_in_order(cx: &mut gpui::TestAppContext) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("first".into()));
    cx.defer_clipboard_reads(true);
    let (zetta, cx) = window(cx);
    send(&zetta, cx, "ctrl-v", None);
    send(&zetta, cx, "x", Some("x"));
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("second".into()));
    send(&zetta, cx, "ctrl-v", None);
    send(&zetta, cx, "y", Some("y"));
    cx.run_until_parked();
    zetta.read_with(cx, |zetta, _| {
        assert_eq!(zetta.command_palette.as_ref().unwrap().query.text, "");
    });
    assert_eq!(cx.complete_clipboard_reads(), 2);
    cx.run_until_parked();
    zetta.read_with(cx, |zetta, _| {
        assert_eq!(
            zetta.command_palette.as_ref().unwrap().query.text,
            "firstxsecondy"
        );
    });
}

#[gpui::test]
fn a_late_paste_cannot_edit_a_reopened_field(cx: &mut gpui::TestAppContext) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("obsolete".into()));
    cx.defer_clipboard_reads(true);
    let (zetta, cx) = window(cx);
    send(&zetta, cx, "ctrl-v", None);
    send(&zetta, cx, "escape", None);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.toggle_command_palette(&ToggleCommandPalette, window, cx);
    });
    send(&zetta, cx, "n", Some("n"));
    cx.complete_clipboard_reads();
    cx.run_until_parked();
    zetta.read_with(cx, |zetta, _| {
        assert_eq!(zetta.command_palette.as_ref().unwrap().query.text, "n");
    });
}

#[test]
fn a_new_field_with_equal_contents_is_not_the_same_paste_target() {
    let original = TextField::new("same");
    let queue = OverlayClipboard {
        target: Some(original.clone()),
        ..Default::default()
    };
    assert!(queue.matches(Some(&original)));
    assert!(!queue.matches(Some(&TextField::new("same"))));
    let mut edited = original.clone();
    edited.move_to_start();
    assert!(!queue.matches(Some(&edited)));
}

#[test]
fn out_of_order_read_completion_does_not_release_the_later_paste() {
    let mut queue = OverlayClipboard::default();
    for id in 1..=2 {
        queue.events.push_back(PendingKey {
            id,
            event: key("ctrl-v", None),
            text: None,
        });
    }
    queue.complete(2, Some("later".into()));
    assert!(queue.events.front().unwrap().text.is_none());
    queue.complete(1, None); // Failure releases the barrier too.
    assert_eq!(queue.events.pop_front().unwrap().text, Some(None));
    assert_eq!(
        queue.events.pop_front().unwrap().text,
        Some(Some("later".into()))
    );
}
