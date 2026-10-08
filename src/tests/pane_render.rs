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

/// A pane of a Zosh session that fell back to SSH says so on the pane itself,
/// in its bottom-left corner, for as long as it is shown — and a pane that did
/// not fall back shows nothing there.
#[gpui::test]
fn a_pane_that_fell_back_to_ssh_is_marked_in_its_corner(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
    });
    let window_size = size(px(800.), px(600.));
    let window = cx.open_window(window_size, |window, cx| {
        let mut config = Config::defaults(None, None);
        // No profiles, so `Zetta::new` opens no tab of its own and spawns no
        // shell; the test builds the one pane it needs.
        config.profiles.clear();
        Zetta::new(
            config,
            None,
            ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    });
    let pane_id = window
        .update(cx, |zetta, _, _| {
            let profile = Profile {
                name: "System".to_owned(),
                command: task::Shell::System,
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::Zetta,
            };
            let pane_id = zetta.next_pane_id;
            zetta.next_pane_id += 1;
            let tab_id = zetta.next_tab_id;
            zetta.next_tab_id += 1;
            zetta.tabs.push(Tab {
                id: tab_id,
                attention_id: tab_id,
                attention: None,
                panes: vec![TerminalPane::new(pane_id, profile).with_label_number(1)],
                pane_indices: HashMap::from([(pane_id, 0)]),
                next_pane_label: 2,
                theme_override: None,
                layout: PaneLayout::Pane(pane_id),
                active_pane: pane_id,
                focus_history: vec![pane_id],
                maximized_pane: None,
                minimized_panes: Vec::new(),
                selected_minimized_pane: None,
                broadcast_input: false,
                silent_mode: false,
                close_policy: TabClosePolicy::Close,
                protected: false,
                shared: false,
                custom_title: None,
                worktree_seed_title: None,
                process_title: None,
                icon: Some(IconName::Terminal),
                icon_override: TabIconOverride::None,
                pinned: false,
                renaming_pane: None,
                rename_buffer: None,
                editing_overlay_pane: None,
                overlay_buffer: None,
                overlay_style_picker: None,
            });
            zetta.active_tab = zetta.tabs.len() - 1;
            pane_id
        })
        .unwrap();
    let mut cx = gpui::VisualTestContext::from_window(*window, cx);
    cx.run_until_parked();
    // `debug_bounds` looks selectors up by `&'static str`.
    let selector: &'static str = format!("pane-transport-fallback-{pane_id}").leak();
    assert!(
        cx.debug_bounds(selector).is_none(),
        "a pane that did not fall back is not marked"
    );

    window
        .update(&mut cx, |zetta, _, cx| {
            let tab = &mut zetta.tabs[zetta.active_tab];
            tab.pane_mut(pane_id).unwrap().transport_fallback =
                Some("The Zosh server never answered".into());
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    let badge = cx
        .debug_bounds(selector)
        .expect("a pane that fell back is marked");
    assert!(
        badge.left() < px(24.),
        "the marker sits on the left: {badge:?}"
    );
    assert!(
        badge.bottom() > window_size.height - px(24.),
        "the marker sits at the bottom: {badge:?}"
    );
    assert!(
        badge.size.width < px(120.),
        "the marker is a chip, not a bar: {badge:?}"
    );
}
