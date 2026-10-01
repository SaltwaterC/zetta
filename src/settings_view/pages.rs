use super::pane_templates::render_pane_templates_page;
use super::projects::render_projects_page;
use super::widgets::{KeymapRowRenderContext, SETTINGS_SCROLLBAR_WIDTH};
use super::*;
use crate::settings_editor::SettingKind;
use crate::settings_ui::configuration_page::{ConfigurationItem, configuration_layout};
use crate::settings_ui::keymap::{
    KeymapRow, KeymapStickyCandidate, compute_keymap_sticky_candidates, keymap_row_data,
    keymap_row_data_for,
};
use ui::sticky_items;

pub(super) fn profile_field(
    label: &'static str,
    control: impl IntoElement,
    colors: &ThemeColors,
) -> AnyElement {
    div()
        .min_w_0()
        .child(
            div()
                .mb_1()
                .text_xs()
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(control)
        .into_any_element()
}

pub(super) fn profile_fields_grid(fields: impl IntoIterator<Item = AnyElement>) -> Div {
    div().mt_3().grid().grid_cols(2).gap_3().children(fields)
}

/// The form controls a page builds itself from.
///
/// `render_settings_page_region` wraps `SettingsFormWidgets`' methods in
/// closures so the page and the modals can be built in separate passes; this
/// bundles the references so a per-page function takes one parameter instead
/// of seven. The pages are built inside a cached view, so the dynamic dispatch
/// costs nothing a frame ever pays for.
pub(crate) struct PageWidgets<'a> {
    pub(crate) scroll_indicator: &'a dyn Fn(String, &ScrollHandle) -> AnyElement,
    pub(crate) text_input: &'a dyn Fn(String, TextField, SettingsInput) -> AnyElement,
    pub(crate) dropdown: &'a dyn Fn(String, String, SettingsDropdown) -> AnyElement,
    pub(crate) setting_row:
        &'a dyn Fn(&'static str, &'static str, SettingsControl, AnyElement) -> AnyElement,
    pub(crate) setting_toggle:
        &'a dyn Fn(SharedString, &'static str, bool, SettingsToggle) -> AnyElement,
    pub(crate) numeric: &'a dyn Fn(ConfigSetting, TextField) -> AnyElement,
    pub(crate) opacity_slider: &'a dyn Fn(f32, OpacityTarget) -> AnyElement,
    /// The theme's error colour, for the validation messages pages show.
    pub(crate) error_color: Hsla,
}

pub(crate) fn render_settings_pages(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    zetta_entity: &gpui::Entity<Zetta>,
    focus_status_access: FocusStatusAccess,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    match editor.page {
        SettingsPage::Configuration => {
            render_configuration_page(editor, colors, handle, focus_status_access, widgets)
        }
        SettingsPage::Themes => render_themes_page(editor, colors, handle, widgets),
        SettingsPage::Keymap => render_keymap_page(editor, colors, handle, zetta_entity, widgets),
        SettingsPage::PaneTemplates => render_pane_templates_page(editor, colors, widgets, handle),
        SettingsPage::Projects => render_projects_page(editor, colors, handle, widgets),
    }
}

/// The row that opens the tab-icon picker, showing the icon new tabs get.
fn default_tab_icon_field(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let current = editor.configuration.default_tab_icon;
    picker_trigger(
        "default-tab-icon-picker-trigger",
        SettingsControl::DefaultTabIconPicker,
        h_flex()
            .gap_2()
            .child(Icon::new(current.unwrap_or(IconName::Dash)))
            .child(current.map_or_else(|| "None".to_owned(), tab_icon_label)),
        editor,
        colors,
        handle,
    )
}

/// The row that opens the font picker, showing the current family in itself.
fn font_family_field(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let current_font = editor.configuration.terminal_font_family.clone();
    picker_trigger(
        "terminal-font-family-picker-trigger",
        SettingsControl::FontPicker,
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(current_font.clone())
            .child(current_font),
        editor,
        colors,
        handle,
    )
}

/// The Configuration page, drawn from [`configuration_layout`] — the same list
/// its tab order is built from.
fn render_configuration_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    focus_status_access: FocusStatusAccess,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    // Read only by the macOS Focus status row.
    #[cfg(not(target_os = "macos"))]
    let _ = focus_status_access;
    let rows = configuration_layout(editor)
        .into_iter()
        .map(|item| match item {
            ConfigurationItem::Heading(section) => {
                section_heading(section.title(), Some(section.description().into()), colors)
            }
            ConfigurationItem::Row(setting) => (widgets.setting_row)(
                setting.label(),
                setting.description(),
                setting.control(),
                with_setting_error(
                    configuration_control(setting, editor, colors, handle, widgets),
                    setting,
                    editor,
                    widgets,
                ),
            ),
            #[cfg(target_os = "macos")]
            ConfigurationItem::FocusStatus => (widgets.setting_row)(
                "macOS Focus status",
                "Allow Zetta to follow Focus status; manual Silent Mode remains available",
                SettingsControl::RequestFocusStatusAccess,
                focus_status_control(editor, colors, handle, focus_status_access),
            ),
            ConfigurationItem::Profile(index) => profile_card(
                index,
                &editor.configuration.profiles[index],
                editor,
                colors,
                handle,
                widgets,
            ),
            ConfigurationItem::AddProfile => add_row(
                DialogButton::new("add-settings-profile", "Add profile", ButtonRole::Secondary)
                    .focused(editor.focused_control == Some(SettingsControl::AddProfile))
                    .render(
                        colors,
                        activate_on_click(handle, SettingsControl::AddProfile),
                    ),
                editor,
                &[SettingsControl::AddProfile],
            ),
        });
    div().children(rows).into_any_element()
}

/// The control a setting's row hosts, drawn by the setting's kind. Its label,
/// description and place in the tab order come from the layout.
fn configuration_control(
    setting: ConfigSetting,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let configuration = &editor.configuration;
    let id = setting.element_id();
    match setting.spec().kind {
        SettingKind::Switch(_) => (widgets.setting_toggle)(
            id,
            setting.label(),
            setting.switch_shown(configuration).unwrap_or(false),
            SettingsToggle::Setting(setting),
        ),
        SettingKind::Choice(_) => (widgets.dropdown)(
            id.to_string(),
            setting
                .choice_label(configuration)
                .unwrap_or_default()
                .to_owned(),
            SettingsDropdown::Setting(setting),
        ),
        SettingKind::Number(spec) => {
            (widgets.numeric)(setting, spec.text.field(configuration).clone())
        }
        SettingKind::Text(spec) => (widgets.text_input)(
            id.to_string(),
            spec.text.field(configuration).clone(),
            SettingsInput::Configuration(ConfigTextField::Setting(setting)),
        ),
        SettingKind::Custom(_) => {
            custom_configuration_control(setting, editor, colors, handle, widgets)
        }
    }
}

/// A setting's control, with why its value cannot be saved beneath it while
/// it cannot be.
fn with_setting_error(
    control: AnyElement,
    setting: ConfigSetting,
    editor: &SettingsEditor,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    match &editor.invalid_setting {
        Some((invalid, message)) if *invalid == setting => v_flex()
            .gap_1()
            .child(control)
            .child(
                div()
                    .text_xs()
                    .text_color(widgets.error_color)
                    .child(message.clone()),
            )
            .into_any_element(),
        _ => control,
    }
}

/// The settings with an editor of their own.
fn custom_configuration_control(
    setting: ConfigSetting,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let configuration = &editor.configuration;
    let dropdown = |label: &str, dropdown: SettingsDropdown| {
        (widgets.dropdown)(setting.element_id().to_string(), label.to_owned(), dropdown)
    };
    match setting {
        ConfigSetting::DefaultProfile => dropdown(
            &configuration.default_profile,
            SettingsDropdown::DefaultProfile,
        ),
        ConfigSetting::LightTheme => dropdown(&configuration.theme, SettingsDropdown::Theme),
        ConfigSetting::DarkTheme => {
            dropdown(&configuration.dark_theme, SettingsDropdown::DarkTheme)
        }
        ConfigSetting::DefaultTabIcon => default_tab_icon_field(editor, colors, handle),
        ConfigSetting::FontFamily => font_family_field(editor, colors, handle),
        ConfigSetting::InactivePaneOpacity => (widgets.opacity_slider)(
            configuration.inactive_pane_opacity,
            OpacityTarget::Configuration,
        ),
        _ => unreachable!("{setting:?} is not a custom setting"),
    }
}

/// The macOS Focus status row: what access Zetta has, and the button that asks
/// for it.
#[cfg(target_os = "macos")]
fn focus_status_control(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    focus_status_access: FocusStatusAccess,
) -> AnyElement {
    h_flex()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(colors.text_muted)
                .child(focus_status_access.label()),
        )
        .child(
            DialogButton::new(
                "settings-request-focus-status-access",
                "Request access",
                ButtonRole::Secondary,
            )
            .compact(true)
            .action_tooltip(
                "Request Focus status access",
                &RequestFocusStatusAccess,
                None,
            )
            .focused(editor.focused_control == Some(SettingsControl::RequestFocusStatusAccess))
            .render(
                colors,
                activate_on_click(handle, SettingsControl::RequestFocusStatusAccess),
            ),
        )
        .into_any_element()
}

/// The Themes page: the extension search, what is installed, and what the last
/// search found.
fn render_themes_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let mut rows = theme_search_rows(editor, colors, handle, widgets);
    rows.extend(installed_theme_extension_rows(editor, colors, handle));
    rows.extend(available_theme_extension_rows(editor, colors, handle));
    div().children(rows).into_any_element()
}

/// The page's heading, the note about what is installed from an extension, and
/// the search field with its button.
fn theme_search_rows(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> Vec<AnyElement> {
    let search = (widgets.text_input)(
        "settings-theme-extension-search".to_owned(),
        editor.theme_extension_query.clone(),
        SettingsInput::ThemeSearch,
    );
    vec![
        section_heading(
            "Download themes from Zed extensions",
            Some("Only declared theme JSON files are installed; other extension features are ignored".into()),
            colors,
        ),
        h_flex()
            .mb_3()
            .child(
                SettingsButton::new(
                    "browse-theme-store".to_owned(),
                    "Browse the Zed themes store",
                    SettingsControl::OpenThemeStore,
                )
                .render(editor, colors, handle),
            )
            .into_any_element(),
        track_focus_scroll(
            h_flex().mb_3().gap_2(),
            editor,
            &[SettingsControl::SearchThemes],
        )
        .child(div().flex_1().child(search))
        .child(
            SettingsButton::new(
                "search-theme-extensions".to_owned(),
                "Search",
                SettingsControl::SearchThemes,
            )
            .loading(editor.theme_extensions_loading)
            .render(editor, colors, handle),
        )
        .into_any_element(),
    ]
}

/// The theme extensions already installed, each with the button that removes
/// it. Nothing is emitted when none are.
fn installed_theme_extension_rows(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Vec<AnyElement> {
    if editor.installed_theme_extensions.is_empty() {
        return Vec::new();
    }
    let busy = editor.theme_extension_downloading.is_some();
    let mut rows = vec![section_heading(
        "Installed from Zed extensions",
        None,
        colors,
    )];
    for installed in &editor.installed_theme_extensions {
        let control = SettingsControl::RemoveTheme(installed.id.clone());
        let removing = editor
            .theme_extension_downloading
            .as_ref()
            .is_some_and(|active| active.as_ref() == installed.id);
        let theme_names = installed.theme_names.join(", ");
        let files = format!(
            "{} theme file{}{}",
            installed.file_count,
            if installed.file_count == 1 { "" } else { "s" },
            if theme_names.is_empty() {
                String::new()
            } else {
                format!(" · {theme_names}")
            }
        );
        rows.push(
            track_focus_scroll(
                card_frame(editor.focused_control.as_ref() == Some(&control), colors),
                editor,
                std::slice::from_ref(&control),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child(card_text(installed.id.clone(), [files.into()], colors))
                    .child(
                        SettingsButton::new(
                            format!("remove-theme-extension-{}", installed.id),
                            "Remove",
                            control.clone(),
                        )
                        .destructive()
                        .confirm_with("Confirm remove")
                        .enabled(!busy)
                        .loading(removing)
                        .render(editor, colors, handle),
                    ),
            )
            .into_any_element(),
        );
    }
    rows
}

/// What the last search found, or the line explaining why the list is empty.
fn available_theme_extension_rows(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> Vec<AnyElement> {
    let mut rows = Vec::new();
    if editor.theme_extensions.is_empty() && !editor.theme_extensions_loading {
        rows.push(empty_state(
            if editor.theme_extensions_searched {
                "No theme extensions match"
            } else {
                "Type a theme name and choose Search"
            },
            colors,
        ));
    }
    for extension in &editor.theme_extensions {
        let control = SettingsControl::InstallTheme(extension.id.clone());
        let downloading = editor
            .theme_extension_downloading
            .as_ref()
            .is_some_and(|active| active == &extension.id);
        let already_installed = editor
            .installed_theme_extensions
            .iter()
            .any(|installed| installed.id == extension.id.as_ref());
        let author = if extension.authors.is_empty() {
            String::new()
        } else {
            format!(" by {}", extension.authors.join(", "))
        };
        let description = extension
            .description
            .clone()
            .unwrap_or_else(|| "Theme extension for Zed".to_owned());
        let details = format!(
            "{} downloads · version {}",
            extension.download_count, extension.version
        );
        rows.push(
            track_focus_scroll(
                card_frame(editor.focused_control.as_ref() == Some(&control), colors),
                editor,
                std::slice::from_ref(&control),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child(card_text(
                        format!("{}{author}", extension.name),
                        [description.into(), details.into()],
                        colors,
                    ))
                    .child(
                        SettingsButton::new(
                            format!("install-theme-extension-{}", extension.id),
                            if already_installed {
                                "Installed"
                            } else {
                                "Install"
                            },
                            control.clone(),
                        )
                        .primary()
                        .enabled(editor.theme_extension_downloading.is_none() && !already_installed)
                        .loading(downloading)
                        .render(editor, colors, handle),
                    ),
            )
            .into_any_element(),
        );
    }
    rows
}

fn render_keymap_page(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    zetta_entity: &gpui::Entity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let &PageWidgets {
        scroll_indicator,
        text_input,
        ..
    } = widgets;
    let row_data = keymap_row_data(editor);
    let row_count = row_data.len();
    let no_results = row_count == 0 && !editor.keymap_search.text.trim().is_empty();

    let row_ctx = KeymapRowRenderContext {
        colors: colors.clone(),
        handle: handle.clone(),
        focused_control: editor.focused_control.clone(),
        focused_input: editor.focused_input,
    };
    let zetta_entity_for_sticky = zetta_entity.clone();
    let rows_list = uniform_list("settings-keymap-list", row_count, move |range, _, _| {
        range
            .map(|row| Zetta::render_keymap_row(&row_data[row], &row_ctx))
            .collect::<Vec<_>>()
    })
    .size_full()
    .track_scroll(&editor.keymap_scroll)
    .with_decoration(sticky_items(
        zetta_entity_for_sticky,
        compute_keymap_sticky_candidates,
        render_keymap_sticky_row,
    ));
    let keymap_scroll = editor.keymap_scroll.0.borrow().base_handle.clone();

    div()
        .flex()
        .flex_col()
        .size_full()
        .child(
            div().flex_none().child(section_heading(
                "Keyboard shortcuts",
                Some(
                    "Type a shortcut into a field, or choose Record to capture one from the \
                     keyboard"
                        .into(),
                ),
                colors,
            )),
        )
        .child(div().flex_none().mb_3().child(text_input(
            "settings-keymap-search".to_owned(),
            editor.keymap_search.clone(),
            SettingsInput::KeymapSearch,
        )))
        .when(no_results, |content| {
            content.child(
                div()
                    .flex_none()
                    .child(empty_state("No bindings match", colors)),
            )
        })
        .child(
            // The scroll indicator is absolutely positioned against this
            // container's padding edge, so the padding is what keeps it off the
            // rows rather than painted over their trailing controls.
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .pr(px(SETTINGS_SCROLLBAR_WIDTH + 2.))
                .child(rows_list)
                .child(scroll_indicator(
                    "settings-keymap-scrollbar".to_owned(),
                    &keymap_scroll,
                )),
        )
        .into_any_element()
}

/// A sticky header above the keymap list: the section header or Add context
/// row the list has scrolled past, drawn by the list's own row builder.
fn render_keymap_sticky_row(
    zetta: &mut Zetta,
    candidate: KeymapStickyCandidate,
    _window: &mut Window,
    cx: &mut Context<Zetta>,
) -> smallvec::SmallVec<[AnyElement; 8]> {
    if !matches!(
        candidate.row,
        KeymapRow::SectionHeader(_) | KeymapRow::AddSection
    ) {
        return smallvec::SmallVec::new();
    }
    let colors = zetta.window_theme(cx).colors().clone();
    let handle = cx.entity().downgrade();
    let Some(editor) = zetta.settings_editor.as_ref() else {
        return smallvec::SmallVec::new();
    };
    let ctx = KeymapRowRenderContext {
        colors,
        handle,
        focused_control: editor.focused_control.clone(),
        focused_input: editor.focused_input,
    };
    keymap_row_data_for(editor, candidate.row)
        .map(|row| Zetta::render_keymap_row(&row, &ctx))
        .into_iter()
        .collect()
}

#[cfg(test)]
#[path = "../tests/settings_view/pages.rs"]
mod tests;

/// One profile's card, tracked for keyboard scrolling as a whole so focusing
/// any control inside it brings the card into view.
///
/// A detected profile and a user-defined one are different cards, not one card
/// with fields disabled: a detected profile's program and arguments come from
/// what is installed and cannot be edited, so it shows them as text and offers
/// only the four overrides.
fn profile_card(
    index: usize,
    profile: &crate::settings_editor::ProfileForm,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let overrides = ProfileOverrides {
        id_prefix: format!("settings-profile-{index}"),
        visibility: SettingsToggle::ProfileVisibility(index),
        icon: SettingsDropdown::ProfileIcon(index),
        theme: SettingsDropdown::ProfileTheme(index),
        dark_theme: SettingsDropdown::ProfileDarkTheme(index),
        profile,
        automatic_icon: &profile.automatic_icon,
    };
    let controls = profile_controls(index, profile.detected, profile.arguments.len());
    let identity = (!profile.detected).then(|| ProfileIdentity {
        id_prefix: format!("settings-profile-{index}"),
        name: SettingsInput::Configuration(ConfigTextField::ProfileName(index)),
        program: SettingsInput::Configuration(ConfigTextField::ProfileProgram(index)),
        program_hint: None,
        remove: SettingsControl::RemoveProfile(index),
        remove_label: "profile",
        arguments: ProfileTarget::Configuration(index),
    });
    render_profile_card(
        ProfileCard {
            profile,
            identity,
            overrides,
            controls: &controls,
        },
        editor,
        colors,
        handle,
        widgets,
    )
}

/// What a profile card shows: the profile, its name/program/arguments fields
/// (absent for a detected profile, which names itself instead), its four
/// overrides, and every control it hosts in tab order, which is what the card
/// highlights for and scrolls into view as one.
///
/// The Configuration page's profiles and a project's profile overrides are
/// both drawn with this; the builder used to draw its overrides as a column of
/// rows labelled "Profile N · …".
pub(super) struct ProfileCard<'a> {
    pub(super) profile: &'a crate::settings_editor::ProfileForm,
    pub(super) identity: Option<ProfileIdentity>,
    pub(super) overrides: ProfileOverrides<'a>,
    pub(super) controls: &'a [SettingsControl],
}

/// The controls of a profile's editable identity: its name, program and
/// arguments, and the button that removes it.
pub(super) struct ProfileIdentity {
    pub(super) id_prefix: String,
    pub(super) name: SettingsInput,
    pub(super) program: SettingsInput,
    /// What leaving the program empty means, where it means something.
    pub(super) program_hint: Option<&'static str>,
    pub(super) remove: SettingsControl,
    pub(super) remove_label: &'static str,
    pub(super) arguments: ProfileTarget,
}

pub(super) fn render_profile_card(
    card: ProfileCard<'_>,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let ProfileCard {
        profile,
        identity,
        overrides,
        controls,
    } = card;
    let focused = controls
        .iter()
        .any(|control| editor.focused_control.as_ref() == Some(control));
    let overrides = profile_override_fields(overrides, colors, handle, widgets.dropdown);
    let card = card_frame(focused, colors);
    let card = match identity {
        Some(identity) => card.child(profile_identity_fields(
            profile, identity, editor, colors, handle, widgets,
        )),
        None => card.child(detected_profile_header(profile, colors)),
    };
    track_focus_scroll(
        div()
            .w_full()
            .child(card.child(profile_fields_grid(overrides))),
        editor,
        controls,
    )
    .into_any_element()
}

/// Which override controls a profile's card hosts, and the profile they show.
pub(super) struct ProfileOverrides<'a> {
    pub(super) id_prefix: String,
    pub(super) visibility: SettingsToggle,
    pub(super) icon: SettingsDropdown,
    pub(super) theme: SettingsDropdown,
    pub(super) dark_theme: SettingsDropdown,
    pub(super) profile: &'a crate::settings_editor::ProfileForm,
    /// The icon shown while the profile has none of its own. A draft's is
    /// worked out from the program as it is typed.
    pub(super) automatic_icon: &'a ProfileIcon,
}

/// The four overrides every profile has, detected or not: whether it is shown
/// in the Profiles menu, its icon, and its two themes. Also what the Add
/// profile modal shows for its draft.
pub(super) fn profile_override_fields(
    overrides: ProfileOverrides<'_>,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    dropdown: &dyn Fn(String, String, SettingsDropdown) -> AnyElement,
) -> [AnyElement; 4] {
    let ProfileOverrides {
        id_prefix,
        visibility,
        icon,
        theme,
        dark_theme,
        profile,
        automatic_icon,
    } = overrides;
    let icon_value = profile.icon.as_ref().unwrap_or(automatic_icon);
    [
        profile_field(
            "Shown in Profiles menu",
            toggle_switch(
                SharedString::from(format!("{id_prefix}-visibility")),
                "Show this profile in the Profiles menu",
                !profile.hidden,
                visibility,
                handle,
            ),
            colors,
        ),
        profile_field(
            "Icon",
            h_flex()
                .w_full()
                .gap_2()
                .child(icon_value.render(IconSize::Small))
                .child(div().min_w_0().flex_1().child(dropdown(
                    format!("{id_prefix}-icon"),
                    ProfileIcon::selector_label(profile.icon.as_ref()).to_owned(),
                    icon,
                ))),
            colors,
        ),
        profile_field(
            "Light theme",
            dropdown(
                format!("{id_prefix}-theme"),
                profile_theme_label(profile.theme.as_deref()).to_owned(),
                theme,
            ),
            colors,
        ),
        profile_field(
            "Dark theme",
            dropdown(
                format!("{id_prefix}-dark-theme"),
                profile_theme_label(profile.dark_theme.as_deref()).to_owned(),
                dark_theme,
            ),
            colors,
        ),
    ]
}

/// What a profile's theme dropdown shows: the theme, or the word for "none of
/// its own" while it follows the application's.
fn profile_theme_label(theme: Option<&str>) -> &str {
    theme.unwrap_or(crate::settings_ui::PROFILE_THEME_INHERIT_LABEL)
}

/// A profile discovered on this machine: its program and arguments come from
/// what is installed, so the card names it rather than editing it.
fn detected_profile_header(
    profile: &crate::settings_editor::ProfileForm,
    colors: &ThemeColors,
) -> AnyElement {
    let icon = profile.icon.as_ref().unwrap_or(&profile.automatic_icon);
    h_flex()
        .min_w_0()
        .flex_1()
        .gap_2()
        .child(icon.render(IconSize::Medium))
        .child(card_text(
            profile.name.text.clone(),
            [SharedString::from("Detected profile")],
            colors,
        ))
        .into_any_element()
}

/// A profile's name, program and arguments, and the button that removes it.
fn profile_identity_fields(
    profile: &crate::settings_editor::ProfileForm,
    identity: ProfileIdentity,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    widgets: &PageWidgets<'_>,
) -> AnyElement {
    let ProfileIdentity {
        id_prefix,
        name,
        program,
        program_hint,
        remove,
        remove_label,
        arguments,
    } = identity;
    let text_input = widgets.text_input;
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .items_end()
                .gap_2()
                .child(div().min_w_0().flex_1().child(profile_field(
                    "Profile name",
                    text_input(format!("{id_prefix}-name"), profile.name.clone(), name),
                    colors,
                )))
                .child(settings_remove_button(
                    editor,
                    format!("{id_prefix}-remove"),
                    remove,
                    remove_label,
                    true,
                    colors,
                    handle,
                )),
        )
        .child(
            div()
                .child(profile_field(
                    "Program",
                    text_input(
                        format!("{id_prefix}-program"),
                        profile.program.clone(),
                        program,
                    ),
                    colors,
                ))
                .when_some(program_hint, |field, hint| {
                    field.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child(hint),
                    )
                }),
        )
        .child(profile_field(
            "Arguments",
            argument_list(
                editor,
                arguments,
                &profile.arguments,
                &id_prefix,
                &editor.settings_scroll,
                colors,
                handle,
            ),
            colors,
        ))
        .into_any_element()
}
