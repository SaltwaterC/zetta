use super::*;

#[test]
fn settings_control_navigation_wraps_and_starts_at_the_expected_end() {
    assert_eq!(adjacent_settings_control_index(0, None, false), None);
    assert_eq!(adjacent_settings_control_index(3, Some(0), true), Some(2));
    assert_eq!(adjacent_settings_control_index(3, Some(2), false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, false), Some(0));
    assert_eq!(adjacent_settings_control_index(3, None, true), Some(2));
}

#[test]
fn dropdown_snapshot_lists_every_option_when_no_query_is_typed() {
    let options = vec![
        "Alpha".to_owned(),
        "A much longer option label".to_owned(),
        "Beta".to_owned(),
    ];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "");

    assert_eq!(rows.as_ref(), [0, 1, 2]);
    // The measured row is an index into `rows`, not into `options`.
    assert_eq!(widest_row, Some(1));
}

#[test]
fn dropdown_snapshot_keeps_fuzzy_matches_and_remeasures_them() {
    let options = vec![
        "zetta::NewTab".to_owned(),
        "zetta::CloseTab".to_owned(),
        "zetta::SplitHorizontalDown".to_owned(),
    ];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "tab");

    assert_eq!(rows.as_ref(), fuzzy_match_indices(&options, "tab"));
    assert_eq!(rows.as_ref(), [0, 1]);
    // Widest among the *matching* rows, so the popover is not sized for a
    // filtered-out option.
    assert_eq!(widest_row, Some(1));
}

#[test]
fn dropdown_snapshot_is_empty_when_nothing_matches() {
    let options = vec!["Alpha".to_owned(), "Beta".to_owned()];

    let (rows, widest_row) = dropdown_snapshot_rows(&options, "gamma");

    assert!(rows.is_empty());
    assert_eq!(widest_row, None);
}

#[test]
fn filtered_dropdown_selection_scrolls_to_its_visible_row() {
    let config = Config::parse("{}", None, None).unwrap();
    let mut editor = configuration_editor(&config);
    editor.open_dropdown_rows = Arc::from([1usize, 4, 9]);
    editor.dropdown_index = 4;

    scroll_open_dropdown_to_selection(&mut editor);

    assert_eq!(editor.dropdown_index, 4);
    assert_eq!(editor.dropdown_scroll.logical_scroll_top_index(), 1);
}

#[test]
fn dropdown_scroll_uses_the_filtered_row_index() {
    let config = Config::parse("{}", None, None).unwrap();
    let mut editor = configuration_editor(&config);
    editor.open_dropdown_rows = Arc::from([4, 17, 29]);
    editor.dropdown_index = 29;

    scroll_open_dropdown_to_selection(&mut editor);

    assert_eq!(editor.dropdown_scroll.logical_scroll_top_index(), 2);
}

#[test]
fn the_form_scroll_mapping_skips_the_controls_that_live_in_the_dialog_header() {
    let controls = vec![
        SettingsControl::Tab(SettingsPage::Configuration),
        SettingsControl::Tab(SettingsPage::Themes),
        SettingsControl::Tab(SettingsPage::Keymap),
        SettingsControl::Tab(SettingsPage::PaneTemplates),
        SettingsControl::Tab(SettingsPage::Projects),
        SettingsControl::Close,
        SettingsControl::Save,
        SettingsControl::SelectPaneTemplate(0),
        SettingsControl::NewPaneTemplate,
    ];

    // The tab strip, Close, and Save are painted in the fixed header, so the
    // first form control has to map to the top of the scroll range: counting
    // them as form controls scrolls every page too far.
    assert_eq!(leading_header_controls(&controls), 7);
    assert_eq!(leading_header_controls(&controls[..7]), 7);
    assert_eq!(leading_header_controls(&[]), 0);
}

#[test]
fn detected_profile_controls_follow_the_visible_selection_order() {
    assert_eq!(
        profile_controls(3, true),
        vec![
            SettingsControl::Toggle(SettingsToggle::ProfileVisibility(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileIcon(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(3)),
        ]
    );
}

#[test]
fn custom_profile_controls_follow_the_visible_selection_order() {
    assert_eq!(
        profile_controls(3, false),
        vec![
            SettingsControl::Input(SettingsInput::Configuration(ConfigTextField::ProfileName(
                3
            ),)),
            SettingsControl::RemoveProfile(3),
            SettingsControl::Input(SettingsInput::Configuration(
                ConfigTextField::ProfileProgram(3),
            )),
            SettingsControl::Input(SettingsInput::Configuration(
                ConfigTextField::ProfileArguments(3),
            )),
            SettingsControl::Toggle(SettingsToggle::ProfileVisibility(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileIcon(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(3)),
            SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(3)),
        ]
    );
}

#[test]
fn profile_draft_controls_include_visibility_and_modal_actions_in_order() {
    assert_eq!(
        profile_draft_controls(),
        [
            SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Name)),
            SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Program)),
            SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Arguments)),
            SettingsControl::Toggle(SettingsToggle::ProfileDraftVisibility),
            SettingsControl::Dropdown(SettingsDropdown::ProfileDraftIcon),
            SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
            SettingsControl::Dropdown(SettingsDropdown::ProfileDraftDarkTheme),
            SettingsControl::Close,
            SettingsControl::CreateProfile,
        ]
    );
}

#[test]
fn dismissing_the_profile_modal_clears_each_draft_dropdown() {
    let config = Config::parse(
        r#"{"profiles":[{"name":"Toolbox","program":"/bin/sh"}]}"#,
        None,
        None,
    )
    .unwrap();
    let mut editor = configuration_editor(&config);

    for dropdown in [
        SettingsDropdown::ProfileDraftTheme,
        SettingsDropdown::ProfileDraftDarkTheme,
        SettingsDropdown::ProfileDraftIcon,
    ] {
        editor.profile_draft = Some(crate::settings_editor::ProfileForm {
            name: TextField::default(),
            program: TextField::default(),
            arguments: TextField::default(),
            theme: None,
            dark_theme: None,
            icon: None,
            automatic_icon: ProfileIcon::Zetta,
            hidden: false,
            detected: false,
        });
        editor.open_dropdown = Some(dropdown);
        editor.dropdown_query = "profile".to_owned();

        editor.dismiss_profile_draft();

        assert!(editor.profile_draft.is_none());
        assert_eq!(editor.open_dropdown, None);
        assert!(editor.dropdown_query.is_empty());
    }
}

#[test]
fn reloading_a_clean_configuration_editor_shows_new_configured_profiles() {
    let path = std::env::temp_dir().join(format!(
        "zetta-settings-profile-refresh-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let old_config = Config::parse("{}", Some(&path), None).unwrap();
    let mut editor = configuration_editor(&old_config);

    std::fs::write(
        &path,
        r#"{"profiles":[{"name":"x","program":"x","args":[] }]}"#,
    )
    .unwrap();
    let new_config = Config::load(Some(&path), None).unwrap();

    editor.refresh_configuration(&new_config).unwrap();

    let profile = editor
        .configuration
        .profiles
        .iter()
        .find(|profile| profile.name.text == "x")
        .expect("the newly configured profile should be in the form");
    assert_eq!(profile.program.text, "x");
    assert!(editor.profile_names.iter().any(|name| name == "x"));

    std::fs::remove_file(path).unwrap();
}

/// A Configuration-page editor with no files behind it: the form falls back to
/// its bundled defaults when the path does not exist, which is all the tab order
/// depends on.
pub(crate) fn configuration_editor(config: &Config) -> SettingsEditor {
    let missing = Path::new("zetta-settings-ui-controls-tests-nonexistent.json");
    SettingsEditor {
        page: SettingsPage::Configuration,
        configuration: crate::settings_editor::ConfigurationForm::load(missing, config).unwrap(),
        keymap: crate::settings_editor::KeymapForm::load(missing).unwrap(),
        profile_names: Arc::from([]),
        themes: Arc::from(["One Dark".to_owned()]),
        theme_extension_query: TextField::default(),
        theme_extensions: Vec::new(),
        installed_theme_extensions: Vec::new(),
        theme_extensions_loading: false,
        theme_extensions_searched: false,
        theme_extension_downloading: None,
        actions: Arc::from([]),
        pane_template_names: Arc::from([]),
        project_roots: Arc::from([]),
        project: None,
        project_loading: false,
        fonts: Arc::from([]),
        normalized_fonts: Arc::from([]),
        font_query: None,
        profile_draft: None,
        keymap_search: TextField::default(),
        settings_scroll: ScrollHandle::new(),
        profile_draft_scroll: ScrollHandle::new(),
        dropdown_scroll: UniformListScrollHandle::new(),
        font_scroll: UniformListScrollHandle::new(),
        keymap_scroll: UniformListScrollHandle::new(),
        numeric_repeat_generation: 0,
        scroll_geometry_initialized: true,
        focused_input: None,
        focused_control: None,
        focus_scroll_request: None,
        keymap_capture: None,
        open_dropdown: None,
        dropdown_index: 0,
        dropdown_query: String::new(),
        dropdown_anchor: Point::default(),
        configuration_dirty: false,
        keymap_dirty: false,
        message: None,
        pane_template_validation_error: None,
        pane_template_validation_generation: 0,
        settings_save_in_progress: false,
        keymap_filtered_sections: None,
        keymap_search_query_cache: String::new(),
        keymap_filtered_bindings: std::collections::HashMap::new(),
        keymap_rows_cache: None,
        keymap_row_data_cache: None,
        open_dropdown_options: Arc::from([]),
        open_dropdown_rows: Arc::from([]),
        open_dropdown_widest_row: None,
        font_filtered_indices: None,
        font_search_query_cache: String::new(),
        controls_cache: None,
        controls_generation: 0,
    }
}

fn position_of(controls: &[SettingsControl], control: &SettingsControl) -> usize {
    controls
        .iter()
        .position(|candidate| candidate == control)
        .unwrap_or_else(|| panic!("{control:?} should be in the Configuration tab order"))
}

/// `scroll_settings_control_into_view` maps a control's position in the tab
/// order onto the scroll range, so the two orders have to agree or focusing a
/// control scrolls somewhere else entirely. The background-session block was
/// drawn between the scrollback setting and the appearance toggles while being
/// tabbed after the pane-control dropdowns, and clicking **Identity file**
/// scrolled it off the top of the dialog.
#[test]
fn the_background_session_controls_are_tabbed_where_the_page_draws_them() {
    let config = Config::parse(
        r#"{"profiles":[{"name":"Toolbox","program":"/bin/sh"}]}"#,
        None,
        None,
    )
    .unwrap();
    let editor = configuration_editor(&config);
    let controls = Zetta::build_settings_controls(&editor);

    let scrollback = position_of(
        &controls,
        &SettingsControl::Numeric(NumericSetting::ScrollHistory),
    );
    let retention = position_of(
        &controls,
        &SettingsControl::Dropdown(SettingsDropdown::SessionRetention),
    );
    let opacity = position_of(&controls, &SettingsControl::Opacity);
    let pane_controls = position_of(
        &controls,
        &SettingsControl::Dropdown(SettingsDropdown::PaneControlsPosition),
    );

    assert!(
        scrollback < retention,
        "the session block is drawn after the scrollback setting"
    );
    assert!(
        retention < opacity,
        "the session block is drawn before the appearance toggles"
    );
    assert!(
        opacity < pane_controls,
        "the appearance toggles are drawn before the pane-control dropdowns"
    );
}

/// The identity field is the one that broke: its estimated scroll position has
/// to land near the row the page draws, not near the end of the form.
#[cfg(feature = "session-persistence")]
#[test]
fn the_persistence_fields_follow_the_session_block_rather_than_the_form_tail() {
    let config = Config::parse(
        r#"{"profiles":[{"name":"Toolbox","program":"/bin/sh"}]}"#,
        None,
        None,
    )
    .unwrap();
    let editor = configuration_editor(&config);
    let controls = Zetta::build_settings_controls(&editor);

    let ring_bytes = position_of(
        &controls,
        &SettingsControl::Numeric(NumericSetting::SessionRingBytes),
    );
    let identity = position_of(
        &controls,
        &SettingsControl::Input(SettingsInput::Configuration(
            ConfigTextField::SessionPersistenceIdentity,
        )),
    );
    let opacity = position_of(&controls, &SettingsControl::Opacity);

    assert!(ring_bytes < identity && identity < opacity);
}

/// Without a recipient and an effective identity the toggle is not drawn, so it
/// must not be a tab stop either — the focus ring would land on nothing.
#[cfg(feature = "session-persistence")]
#[test]
fn the_automatic_protection_toggle_joins_the_tab_order_only_when_it_is_drawn() {
    let config = Config::parse(
        r#"{"profiles":[{"name":"Toolbox","program":"/bin/sh"}]}"#,
        None,
        None,
    )
    .unwrap();
    let mut editor = configuration_editor(&config);
    let toggle = SettingsControl::Toggle(SettingsToggle::SessionAutoProtect);
    assert!(!Zetta::build_settings_controls(&editor).contains(&toggle));

    editor.configuration.session_persistence_recipients = TextField::new("age1example".to_owned());
    editor.configuration.session_persistence_identity = TextField::new("~/keys/id.txt".to_owned());
    let controls = Zetta::build_settings_controls(&editor);
    assert!(controls.contains(&toggle));

    // And it stays with the block it belongs to.
    let identity = position_of(
        &controls,
        &SettingsControl::Input(SettingsInput::Configuration(
            ConfigTextField::SessionPersistenceIdentity,
        )),
    );
    assert_eq!(position_of(&controls, &toggle), identity + 1);
}

/// The tab row and this tab order are built from separate lists — one in
/// `settings_view`, one in `build_settings_controls` — so a page added to the
/// row without a control here is reachable by mouse and not by keyboard, and a
/// control without a row focuses a tab that is never drawn.
#[test]
fn the_keyboard_tab_order_matches_the_tab_row_the_dialog_draws() {
    let config = Config::parse("{}", None, None).unwrap();
    let mut editor = configuration_editor(&config);
    let focusable: Vec<SettingsPage> = Zetta::settings_controls(&mut editor)
        .into_iter()
        .filter_map(|control| match control {
            SettingsControl::Tab(page) => Some(page),
            _ => None,
        })
        .collect();
    let drawn: Vec<SettingsPage> = crate::settings_view::SETTINGS_PAGE_TABS
        .iter()
        .map(|(page, ..)| *page)
        .collect();
    assert_eq!(drawn, focusable);
}
