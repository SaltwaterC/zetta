use super::*;
use gpui::Modifiers;
use terminal::{PathLikeTarget, is_hyperlink_modifier};
use util::paths::PathStyle;

#[derive(Clone, Default)]
struct RecordedStreamInput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl RecordedStreamInput {
    fn bytes(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl std::io::Write for RecordedStreamInput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[gpui::test]
async fn ctrl_c_copies_selection_before_forwarding_interrupt(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
        cx.bind_keys([gpui::KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Terminal && selection"),
        )]);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        let mut view = TerminalView::new(terminal, window, cx);
        view.set_emit_input_events(true);
        view.focus_handle.focus(window, cx);
        view
    });
    let interrupts = std::rc::Rc::new(std::cell::Cell::new(0));
    let received = interrupts.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let TerminalViewEvent::Input(TerminalInput::Keystroke(key)) = event {
                assert_eq!(key, &Keystroke::parse("ctrl-c").unwrap());
                received.set(received.get() + 1);
            }
        })
        .detach();
    });
    cx.run_until_parked();
    view.update_in(cx, |view, _window, cx| {
        view.terminal.update(cx, |terminal, cx| {
            terminal.write_output(
                b"\x1b[?1049h\x1b[?1000h\x1b[?1006hselected terminal output\r\n",
                cx,
            );
            terminal.select_all();
        });
        cx.notify();
    });
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        interrupts.get(),
        0,
        "copy must not interrupt the application"
    );
    let clipboard = cx.update(|_, cx| terminal::selection_clipboard::read(cx));
    assert!(view.update(cx, |view, cx| {
        view.terminal.read(cx).last_content.selection.is_none()
    }));
    assert_eq!(
        clipboard
            .await
            .and_then(|item| item.text())
            .as_deref()
            .map(str::trim_end),
        Some("selected terminal output")
    );
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        interrupts.get(),
        1,
        "Ctrl+C without selection reaches the application"
    );
}

#[gpui::test]
async fn normal_drag_in_mouse_mode_leaves_ctrl_c_with_the_application(
    cx: &mut gpui::TestAppContext,
) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
        cx.bind_keys([gpui::KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Terminal && selection"),
        )]);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("original".to_owned()));
    });
    let input = RecordedStreamInput::default();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            terminal::TerminalBuilder::new_byte_stream(
                Box::new(std::io::empty()),
                Box::new(input.clone()),
                String::new(),
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        let mut view = TerminalView::new(terminal, window, cx);
        view.set_emit_input_events(true);
        view.focus_handle.focus(window, cx);
        view
    });
    let copies = std::rc::Rc::new(std::cell::Cell::new(0));
    let received = copies.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let TerminalViewEvent::Input(TerminalInput::Keystroke(key)) = event {
                assert_eq!(key, &Keystroke::parse("ctrl-c").unwrap());
                received.set(received.get() + 1);
            }
        })
        .detach();
    });
    view.update(cx, |view, cx| {
        view.terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"\x1b[?1049h\x1b[?1002h\x1b[?1006hOpenAI Codex\r\n", cx);
        });
        cx.notify();
    });
    cx.run_until_parked();
    let bounds = view.read_with(cx, |view, cx| view.terminal_bounds(cx));
    let start = bounds.bounds.origin + gpui::point(bounds.cell_width / 2., bounds.line_height / 2.);
    let end = start + gpui::point(bounds.cell_width * 10., gpui::px(0.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(end, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, Modifiers::none());
    assert!(view.read_with(cx, |view, cx| {
        view.terminal.read(cx).last_content.selection.is_none()
    }));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        copies.get(),
        1,
        "the application owns the normal-drag selection"
    );
    let expected = b"\x1b[<0;1;1M\x1b[<32;11;1M\x1b[<0;11;1m\x03";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while input.bytes().len() < expected.len() && std::time::Instant::now() < deadline {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        input.bytes(),
        expected,
        "mouse selection and Ctrl+C reach the remote stream"
    );
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("original"),
        "an application's mouse selection is not a terminal-grid selection"
    );
}

#[test]
fn pane_customization_context_menu_entries_have_stable_order() {
    let entries = pane_customization_context_menu_entries();

    assert_eq!(
        entries.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        ["Change Pane Theme", "Set Pane Overlay"]
    );
    assert_eq!(
        entries
            .iter()
            .map(|(_, action)| action.name())
            .collect::<Vec<_>>(),
        ["zetta::ChangePaneTheme", "zetta::SetPaneOverlay"]
    );
}

#[test]
fn trimmed_paste_removes_only_outer_whitespace() {
    assert_eq!(
        trim_paste_text(" \t\r\n first line \n second line \r\n\t "),
        "first line \n second line"
    );
}

#[test]
fn search_caret_respects_utf8_boundaries() {
    let text = "a\u{e9}\u{4e2d}";
    assert_eq!(next_char_boundary(text, 1), 3);
    assert_eq!(previous_char_boundary(text, 3), 1);
}

#[test]
fn disabled_input_events_are_not_allocated() {
    let builds = std::cell::Cell::new(0);
    let event = enabled_input_event(false, || {
        builds.set(builds.get() + 1);
        super::TerminalViewEvent::Input(super::TerminalInput::Text("ignored".into()))
    });
    assert!(event.is_none());
    assert_eq!(builds.get(), 0);
}

#[test]
fn silent_mode_only_gates_the_system_bell() {
    assert!(should_play_system_bell(true, TerminalBell::System));
    assert!(!should_play_system_bell(false, TerminalBell::System));
    assert!(!should_play_system_bell(true, TerminalBell::Off));
}

#[test]
fn shifted_right_click_opens_the_context_menu() {
    assert_eq!(
        right_click_action(true, true, true, true),
        RightClickAction::ContextMenu
    );
    assert_eq!(
        right_click_action(true, true, true, false),
        RightClickAction::ContextMenu
    );
}

#[test]
fn plain_right_click_pastes_when_clipboard_has_content() {
    assert_eq!(
        right_click_action(false, false, true, true),
        RightClickAction::Paste
    );
}

#[test]
fn the_first_nonempty_clipboard_image_is_selected_for_paste() {
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![1, 2, 3]);
    let clipboard = gpui::ClipboardItem {
        entries: vec![
            gpui::ClipboardEntry::Image(gpui::Image::empty()),
            gpui::ClipboardEntry::String("text alongside image".to_owned().into()),
            gpui::ClipboardEntry::Image(image.clone()),
        ],
    };

    assert_eq!(first_clipboard_image(&clipboard).as_deref(), Some(&image));
    assert!(
        first_clipboard_image(&gpui::ClipboardItem {
            entries: vec![gpui::ClipboardEntry::Image(gpui::Image::empty())],
        })
        .is_none()
    );
}

#[test]
fn plain_right_click_opens_the_context_menu_without_clipboard_text() {
    assert_eq!(
        right_click_action(false, false, true, false),
        RightClickAction::ContextMenu
    );
}

#[test]
fn plain_right_click_opens_the_context_menu_when_right_click_paste_is_off() {
    assert_eq!(
        right_click_action(false, false, false, true),
        RightClickAction::ContextMenu
    );
    // Turning paste off does not take the click away from an application
    // that asked for mouse reports.
    assert_eq!(
        right_click_action(true, false, false, true),
        RightClickAction::Forward
    );
}

#[test]
fn plain_right_click_is_forwarded_in_terminal_mouse_mode() {
    assert_eq!(
        right_click_action(true, false, true, true),
        RightClickAction::Forward
    );
}

#[test]
fn control_is_a_hyperlink_modifier_on_every_platform() {
    assert!(is_hyperlink_modifier(&Modifiers {
        control: true,
        ..Default::default()
    }));
}

#[test]
fn search_status_marks_a_count_that_is_still_growing() {
    let label = |active_match, shown, total_count, limit_reached, complete| {
        SearchStatus {
            active_match,
            shown,
            total_count,
            limit_reached,
            complete,
        }
        .label()
    };

    assert_eq!(label(None, 0, 0, false, false), "Searching\u{2026}");
    assert_eq!(label(None, 0, 0, false, true), "0 / 0");
    assert_eq!(label(Some(2), 3, 3, false, false), "3 / 3+");
    assert_eq!(label(Some(2), 3, 3, false, true), "3 / 3");
    assert_eq!(
        label(Some(255), 256, 9_000, true, false),
        "256 / 256 shown \u{b7} 9000+ matches"
    );
    assert_eq!(
        label(Some(255), 256, 9_000, true, true),
        "256 / 256 shown \u{b7} 9000 matches"
    );
}

pub(super) fn link_test_view(
    cx: &mut gpui::TestAppContext,
) -> (Entity<TerminalView>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
    });
    cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        TerminalView::new(terminal, window, cx)
    })
}

fn emit_terminal_event(
    view: &Entity<TerminalView>,
    event: Event,
    cx: &mut gpui::VisualTestContext,
) {
    view.update_in(cx, |view, _, cx| {
        view.terminal.update(cx, |_, cx| cx.emit(event));
    });
    cx.run_until_parked();
}

/// The terminal asks for confirmation for a link whose scheme an OS handler
/// would otherwise receive unasked; the view must not open it until the user
/// agrees, and must not open it at all when they decline.
#[gpui::test]
async fn a_link_needing_confirmation_opens_only_once_the_user_agrees(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx) = link_test_view(cx);
    let url = "vscode://file/\u{202E}txt.exe".to_owned();

    emit_terminal_event(&view, Event::ConfirmOpenUrl(url.clone()), cx);
    let (_, detail) = cx.pending_prompt().expect("the user must be asked first");
    assert!(
        detail.starts_with("vscode://file/\\u{202E}txt.exe"),
        "the prompt shows the destination with its bidi override escaped: {detail:?}"
    );
    assert_eq!(cx.opened_url(), None);
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(cx.opened_url(), None, "a declined link must not be opened");

    emit_terminal_event(&view, Event::ConfirmOpenUrl(url.clone()), cx);
    cx.simulate_prompt_answer("Open");
    cx.run_until_parked();
    assert_eq!(cx.opened_url(), Some(url));
}

#[gpui::test]
async fn a_web_link_opens_without_asking(cx: &mut gpui::TestAppContext) {
    let (view, cx) = link_test_view(cx);
    emit_terminal_event(
        &view,
        Event::Open(MaybeNavigationTarget::Url(
            "https://example.com/".to_owned(),
        )),
        cx,
    );
    assert!(!cx.has_pending_prompt());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/"));
}

#[gpui::test]
async fn a_hover_tooltip_shows_format_characters_escaped(cx: &mut gpui::TestAppContext) {
    let (view, cx) = link_test_view(cx);
    view.update_in(cx, |view, _, cx| {
        view.terminal.update(cx, |terminal, _| {
            terminal.last_content.last_hovered_word = Some(HoveredWord {
                word: "click me".to_owned(),
                word_match: terminal::Range::new(
                    terminal::Point::new(0, 0),
                    terminal::Point::new(0, 7),
                ),
                id: 0,
            });
        });
    });
    emit_terminal_event(
        &view,
        Event::NewNavigationTarget(Some(MaybeNavigationTarget::Url(
            "https://example.com/\u{2066}evil\u{2069}".to_owned(),
        ))),
        cx,
    );
    let tooltip = view.read_with(cx, |view, _| {
        view.hover.as_ref().map(|hover| hover.tooltip.clone())
    });
    assert_eq!(
        tooltip.as_deref(),
        Some("https://example.com/\\u{2066}evil\\u{2069}")
    );

    emit_terminal_event(
        &view,
        Event::NewNavigationTarget(Some(MaybeNavigationTarget::PathLike(PathLikeTarget {
            maybe_path: "/tmp/\u{202E}gpj.sh".to_owned(),
            terminal_dir: None,
            path_style: PathStyle::local(),
            spaced_readings: Vec::new(),
            host_root: None,
        }))),
        cx,
    );
    let tooltip = view.read_with(cx, |view, _| {
        view.hover.as_ref().map(|hover| hover.tooltip.clone())
    });
    assert_eq!(tooltip.as_deref(), Some("/tmp/\\u{202E}gpj.sh"));
}

/// Hovering one fragment of a name with spaces in it widens the link to the
/// whole name once a background check finds it on disk, so the underline and
/// tooltip say what a click will open.
#[gpui::test]
async fn hovering_a_fragment_of_an_existing_spaced_name_widens_the_link(
    cx: &mut gpui::TestAppContext,
) {
    let directory = std::env::temp_dir().join(format!("zetta-spaced-hover-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("Q3 report.xlsx"), "").unwrap();
    let fragment = terminal::Range::new(terminal::Point::new(0, 3), terminal::Point::new(0, 13));
    let whole = terminal::Range::new(terminal::Point::new(0, 0), terminal::Point::new(0, 13));
    let hover =
        |view: &Entity<TerminalView>, readings: &[&str], cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |view, _, cx| {
                view.terminal.update(cx, |terminal, _| {
                    terminal.last_content.last_hovered_word = Some(HoveredWord {
                        word: "report.xlsx".to_owned(),
                        word_match: fragment,
                        id: 7,
                    });
                });
            });
            emit_terminal_event(
                view,
                Event::NewNavigationTarget(Some(MaybeNavigationTarget::PathLike(PathLikeTarget {
                    maybe_path: "report.xlsx".to_owned(),
                    terminal_dir: Some(directory.clone()),
                    path_style: PathStyle::local(),
                    spaced_readings: readings
                        .iter()
                        .map(|reading| terminal::SpacedPathReading {
                            maybe_path: (*reading).to_owned(),
                            range: whole,
                        })
                        .collect(),
                    host_root: None,
                }))),
                cx,
            );
            view.read_with(cx, |view, cx| {
                let hovered = view
                    .terminal
                    .read(cx)
                    .last_content
                    .last_hovered_word
                    .clone();
                let tooltip = view.hover.as_ref().map(|hover| hover.tooltip.clone());
                (
                    hovered.map(|hovered| (hovered.word, hovered.word_match)),
                    tooltip,
                )
            })
        };
    let (view, cx) = link_test_view(cx);

    assert_eq!(
        hover(&view, &["Q3 report.xlsx"], cx),
        (
            Some(("Q3 report.xlsx".to_owned(), whole)),
            Some("Q3 report.xlsx".to_owned())
        )
    );
    assert_eq!(
        hover(&view, &["Q4 report.xlsx"], cx),
        (
            Some(("report.xlsx".to_owned(), fragment)),
            Some("report.xlsx".to_owned())
        )
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[gpui::test]
fn explicit_backing_cells_reach_the_rendered_terminal(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
        cx.set_global(PhysicalCellSize {
            width: 14,
            height: 28,
        });
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        TerminalView::new(terminal, window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let bounds = view.read(cx).terminal.read(cx).local_terminal_bounds();
        assert_eq!(f32::from(bounds.cell_width) * window.scale_factor(), 14.0);
        assert_eq!(f32::from(bounds.line_height) * window.scale_factor(), 28.0);
        assert!(bounds.num_columns() > 2);
        assert!(bounds.num_lines() > 1);
    });
}
