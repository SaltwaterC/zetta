use super::pane_templates::render_pane_templates_page;
use super::projects::render_projects_page;
use super::widgets::{KEYMAP_ROW_HEIGHT, KeymapRowRenderContext, SETTINGS_SCROLLBAR_WIDTH};
use super::*;
use crate::settings_ui::keymap::{compute_keymap_sticky_candidates, keymap_row_data};
use ui::sticky_items;

fn profile_field(
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

fn profile_fields_grid(fields: impl IntoIterator<Item = AnyElement>) -> Div {
    div().mt_3().grid().grid_cols(2).gap_3().children(fields)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_settings_pages(
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
    zetta_entity: &gpui::Entity<Zetta>,
    focus_status_access: FocusStatusAccess,
    scroll_indicator: &impl Fn(String, &ScrollHandle) -> AnyElement,
    text_input: &impl Fn(String, TextField, SettingsInput) -> AnyElement,
    dropdown: &impl Fn(String, String, SettingsDropdown) -> AnyElement,
    setting_row: &impl Fn(&'static str, &'static str, SettingsControl, AnyElement) -> AnyElement,
    setting_toggle: &impl Fn(&'static str, bool, SettingsToggle) -> AnyElement,
    numeric: &impl Fn(&'static str, TextField, NumericSetting, ConfigTextField) -> AnyElement,
    opacity_slider: &impl Fn(f32, OpacityTarget) -> AnyElement,
) -> AnyElement {
    #[cfg(not(target_os = "macos"))]
    let _ = focus_status_access;

    match editor.page {
        SettingsPage::Configuration => {
            let configuration = &editor.configuration;
            let default_profile = dropdown(
                "settings-default-profile".to_owned(),
                configuration.default_profile.clone(),
                SettingsDropdown::DefaultProfile,
            );
            let new_tab_profile = dropdown(
                "settings-new-tab-profile".to_owned(),
                configuration.new_tab_profile.label().to_owned(),
                SettingsDropdown::NewTabProfile,
            );
            let theme = dropdown(
                "settings-theme".to_owned(),
                configuration.theme.clone(),
                SettingsDropdown::Theme,
            );
            let dark_theme = dropdown(
                "settings-dark-theme".to_owned(),
                configuration.dark_theme.clone(),
                SettingsDropdown::DarkTheme,
            );
            let current_default_tab_icon = configuration.default_tab_icon;
            let default_tab_icon_handle = handle.clone();
            let default_tab_icon = h_flex()
                .id("default-tab-icon-picker-trigger")
                .h_9()
                .w_full()
                .min_w(px(180.))
                .px_3()
                .justify_between()
                .rounded(px(4.))
                .border_1()
                .border_color(
                    if editor.focused_control == Some(SettingsControl::DefaultTabIconPicker) {
                        colors.border_focused
                    } else {
                        colors.border
                    },
                )
                .bg(colors.editor_background)
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(
                            current_default_tab_icon.unwrap_or(IconName::Dash),
                        ))
                        .child(
                            current_default_tab_icon
                                .map(tab_icon_label)
                                .unwrap_or_else(|| "None".to_owned()),
                        ),
                )
                .child(
                    svg()
                        .path(IconName::ChevronDown.path())
                        .size(px(14.))
                        .text_color(colors.icon_muted),
                )
                .on_click(move |_, window, cx| {
                    default_tab_icon_handle
                        .update(cx, |this, cx| {
                            this.open_default_tab_icon_picker(window, cx);
                        })
                        .ok();
                })
                .into_any_element();
            let working_directory_scope = dropdown(
                "settings-working-directory-scope".to_owned(),
                configuration.working_directory_scope.label().to_owned(),
                SettingsDropdown::WorkingDirectoryScope,
            );
            let pane_controls_position = dropdown(
                "settings-pane-controls-position".to_owned(),
                configuration.pane_controls_position.label().to_owned(),
                SettingsDropdown::PaneControlsPosition,
            );
            let pane_controls_default_visibility = dropdown(
                "settings-pane-controls-default-visibility".to_owned(),
                if configuration.pane_controls_hidden_by_default {
                    "Hidden".to_owned()
                } else {
                    "Visible".to_owned()
                },
                SettingsDropdown::PaneControlsDefaultVisibility,
            );
            let session_retention = dropdown(
                "settings-session-retention".to_owned(),
                configuration.session_retention.label().to_owned(),
                SettingsDropdown::SessionRetention,
            );
            let current_font = configuration.terminal_font_family.clone();
            let picker_handle = handle.clone();
            let font_family = h_flex()
                .id("terminal-font-family-picker-trigger")
                .h_9()
                .w_full()
                .min_w(px(180.))
                .px_3()
                .justify_between()
                .rounded(px(4.))
                .border_1()
                .border_color(
                    if editor.focused_control == Some(SettingsControl::FontPicker) {
                        colors.border_focused
                    } else {
                        colors.border
                    },
                )
                .bg(colors.editor_background)
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family(current_font.clone())
                        .child(current_font),
                )
                .child(
                    svg()
                        .path(IconName::ChevronDown.path())
                        .size(px(14.))
                        .text_color(colors.icon_muted),
                )
                .on_click(move |_, window, cx| {
                    picker_handle
                        .update(cx, |this, cx| {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.font_query = Some(TextField::default());
                                editor.scroll_geometry_initialized = false;
                            }
                            this.focus_settings_input(SettingsInput::FontSearch, window, cx);
                        })
                        .ok();
                })
                .into_any_element();
            let mut rows = vec![
                setting_row(
                    "Default profile",
                    "Profile selected when Zetta starts",
                    SettingsControl::Dropdown(SettingsDropdown::DefaultProfile),
                    default_profile,
                ),
                setting_row(
                    "New Tab profile",
                    "Default uses the Default profile above; Inherit reuses the active tab's profile",
                    SettingsControl::Dropdown(SettingsDropdown::NewTabProfile),
                    new_tab_profile,
                ),
                setting_row(
                    "Light theme",
                    "Application color theme used in light appearance",
                    SettingsControl::Dropdown(SettingsDropdown::Theme),
                    theme,
                ),
                setting_row(
                    "Dark theme",
                    "Application color theme used in dark appearance",
                    SettingsControl::Dropdown(SettingsDropdown::DarkTheme),
                    dark_theme,
                ),
                setting_row(
                    "Default tab icon",
                    "Icon shown on new tabs; choose None to hide it",
                    SettingsControl::DefaultTabIconPicker,
                    default_tab_icon,
                ),
                setting_row(
                    "Terminal font size",
                    "Point size from 6 through 100",
                    SettingsControl::Numeric(NumericSetting::FontSize),
                    numeric(
                        "settings-font-size",
                        configuration.terminal_font_size.clone(),
                        NumericSetting::FontSize,
                        ConfigTextField::FontSize,
                    ),
                ),
                setting_row(
                    "Terminal font family",
                    "Search bundled and system-installed font families",
                    SettingsControl::FontPicker,
                    font_family,
                ),
                setting_row(
                    "Working directory",
                    "Initial directory; ~ expands to your home directory",
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::WorkingDirectory,
                    )),
                    text_input(
                        "settings-working-directory".to_owned(),
                        configuration.working_directory.clone(),
                        SettingsInput::Configuration(ConfigTextField::WorkingDirectory),
                    ),
                ),
                setting_row(
                    "Inherit working directory scope",
                    "Choose which new shells inherit the active pane's current directory",
                    SettingsControl::Dropdown(SettingsDropdown::WorkingDirectoryScope),
                    working_directory_scope,
                ),
                setting_row(
                    "Scrollback history",
                    "Enter 0 through Max; steppers accelerate across the range",
                    SettingsControl::Numeric(NumericSetting::ScrollHistory),
                    numeric(
                        "settings-scroll-history",
                        configuration.max_scroll_history_lines.clone(),
                        NumericSetting::ScrollHistory,
                        ConfigTextField::ScrollHistory,
                    ),
                ),
                div()
                    .pt_4()
                    .pb_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(colors.text_muted)
                            .child("Background sessions (zmux)"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child("Screen retention for detached and shared sessions"),
                    )
                    .into_any_element(),
                setting_row(
                    "Detached session retention",
                    "Keep no screen, a bounded in-memory screen, or encrypted disk state; a temporary GitHub outage uses memory until persistence returns",
                    SettingsControl::Dropdown(SettingsDropdown::SessionRetention),
                    session_retention,
                ),
                setting_row(
                    "Detached session ring bytes",
                    "Memory budget for the bounded screen retained by background sessions",
                    SettingsControl::Numeric(NumericSetting::SessionRingBytes),
                    numeric(
                        "settings-session-ring-bytes",
                        configuration.session_ring_bytes.clone(),
                        NumericSetting::SessionRingBytes,
                        ConfigTextField::SessionRingBytes,
                    ),
                ),
                #[cfg(feature = "session-persistence")]
                setting_row(
                    "Disk recipients",
                    "Comma-separated age recipients or github:USER entries used for encrypted session retention",
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::SessionPersistenceRecipients,
                    )),
                    text_input(
                        "settings-session-persistence-recipients".to_owned(),
                        configuration.session_persistence_recipients.clone(),
                        SettingsInput::Configuration(ConfigTextField::SessionPersistenceRecipients),
                    ),
                ),
                #[cfg(feature = "session-persistence")]
                setting_row(
                    "Identity file",
                    "Optional age identity path; ~/.ssh/id_ed25519 is used when this is blank and present",
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::SessionPersistenceIdentity,
                    )),
                    text_input(
                        "settings-session-persistence-identity".to_owned(),
                        configuration.session_persistence_identity.clone(),
                        SettingsInput::Configuration(ConfigTextField::SessionPersistenceIdentity),
                    ),
                ),
            ];
            // Drawn only with a recipient and an effective identity to hand,
            // under the same predicate that decides whether it is a tab stop,
            // so what is on screen and what the keyboard reaches cannot disagree.
            #[cfg(feature = "session-persistence")]
            if configuration.session_auto_protect_is_offered() {
                rows.push(setting_row(
                    "Protect sessions with your key",
                    "Detach, keep and share without a secret prompt: the session key is sealed to \
                     the recipients above and reopened with the identity file",
                    SettingsControl::Toggle(SettingsToggle::SessionAutoProtect),
                    setting_toggle(
                        "settings-session-persistence-auto-protect",
                        configuration.session_persistence_auto_protect,
                        SettingsToggle::SessionAutoProtect,
                    ),
                ));
            }
            rows.extend([
                setting_row(
                    "Inactive pane opacity",
                    "Dimming level as a percentage",
                    SettingsControl::Opacity,
                    opacity_slider(
                        configuration.inactive_pane_opacity,
                        OpacityTarget::Configuration,
                    ),
                ),
                setting_row(
                    "Compact mode",
                    "Move tabs into the title bar and reduce its controls",
                    SettingsControl::Toggle(SettingsToggle::CompactMode),
                    setting_toggle(
                        "settings-compact-mode",
                        configuration.compact_mode,
                        SettingsToggle::CompactMode,
                    ),
                ),
                setting_row(
                    "Hide pane size",
                    "Hide the active pane dimensions from the title bar",
                    SettingsControl::Toggle(SettingsToggle::PaneSize),
                    setting_toggle(
                        "settings-hide-pane-size",
                        configuration.hide_pane_size,
                        SettingsToggle::PaneSize,
                    ),
                ),
                setting_row(
                    "Hide title bar labels",
                    "Hide text such as Menu, Profile, and Keep running when available",
                    SettingsControl::Toggle(SettingsToggle::TitleBarLabels),
                    setting_toggle(
                        "settings-hide-title-bar-labels",
                        configuration.hide_title_bar_labels,
                        SettingsToggle::TitleBarLabels,
                    ),
                ),
                setting_row(
                    "Hide title bar buttons",
                    "Hide title bar buttons such as Keep running in --no-mux mode and Detach",
                    SettingsControl::Toggle(SettingsToggle::TitleBarButtons),
                    setting_toggle(
                        "settings-hide-title-bar-buttons",
                        configuration.hide_title_bar_buttons,
                        SettingsToggle::TitleBarButtons,
                    ),
                ),
                #[cfg(target_os = "macos")]
                setting_row(
                    "Hide title bar menus",
                    "Hide the Menu and Profile menus from the title bar",
                    SettingsControl::Toggle(SettingsToggle::TitleBarMenus),
                    setting_toggle(
                        "settings-hide-title-bar-menus",
                        configuration.hide_title_bar_menus,
                        SettingsToggle::TitleBarMenus,
                    ),
                ),
                #[cfg(target_os = "macos")]
                setting_row(
                    "macOS Focus status",
                    "Allow Zetta to follow Focus status; manual Silent Mode remains available",
                    SettingsControl::RequestFocusStatusAccess,
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
                            Button::new("settings-request-focus-status-access", "Request access")
                                .style(ButtonStyle::Outlined)
                                .size(ButtonSize::Compact)
                                .color(Color::Custom(colors.text))
                                .aria_label("Request macOS Focus status access")
                                .tooltip(Tooltip::for_action_title(
                                    "Request Focus Status Access",
                                    &RequestFocusStatusAccess,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(RequestFocusStatusAccess), cx);
                                }),
                        )
                        .into_any_element(),
                ),
                setting_row(
                    "Pane controls position",
                    "Keep pane overlay controls on the right or move them to the left",
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsPosition),
                    pane_controls_position,
                ),
                setting_row(
                    "Pane controls by default",
                    "Start new panes with overlay controls visible or hidden",
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsDefaultVisibility),
                    pane_controls_default_visibility,
                ),
            ]);
            #[cfg(any(feature = "http-server", feature = "tftp-server"))]
            rows.push(
                div()
                    .pt_4()
                    .pb_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(colors.text_muted)
                            .child("Network services"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child("Ports used by the optional local servers"),
                    )
                    .into_any_element(),
            );
            #[cfg(feature = "http-server")]
            rows.push(setting_row(
                "HTTP server port",
                "TCP port used when starting the static HTTP server",
                SettingsControl::Numeric(NumericSetting::HttpServerPort),
                numeric(
                    "settings-http-server-port",
                    configuration.http_server_port.clone(),
                    NumericSetting::HttpServerPort,
                    ConfigTextField::HttpServerPort,
                ),
            ));
            #[cfg(feature = "tftp-server")]
            rows.push(setting_row(
                "TFTP server port",
                "UDP port used when starting the TFTP server",
                SettingsControl::Numeric(NumericSetting::TftpServerPort),
                numeric(
                    "settings-tftp-server-port",
                    configuration.tftp_server_port.clone(),
                    NumericSetting::TftpServerPort,
                    ConfigTextField::TftpServerPort,
                ),
            ));
            rows.push(
                div()
                    .pt_4()
                    .pb_2()
                    .text_sm()
                    .text_color(colors.text_muted)
                    .child("Profiles")
                    .into_any_element(),
            );
            for (index, profile) in configuration.profiles.iter().enumerate() {
                let profile_controls = profile_controls(index, profile.detected);
                let profile_focused = profile_controls
                    .iter()
                    .any(|control| editor.focused_control.as_ref() == Some(control));
                let profile_theme = profile
                    .theme
                    .clone()
                    .unwrap_or_else(|| "Use application theme".to_owned());
                let profile_theme = dropdown(
                    format!("settings-profile-{index}-theme"),
                    profile_theme,
                    SettingsDropdown::ProfileTheme(index),
                );
                let profile_dark_theme = profile
                    .dark_theme
                    .clone()
                    .unwrap_or_else(|| "Use application theme".to_owned());
                let profile_dark_theme = dropdown(
                    format!("settings-profile-{index}-dark-theme"),
                    profile_dark_theme,
                    SettingsDropdown::ProfileDarkTheme(index),
                );
                let profile_icon_value = profile.icon.as_ref().unwrap_or(&profile.automatic_icon);
                let profile_icon = h_flex()
                    .w_full()
                    .gap_2()
                    .child(profile_icon_value.render(IconSize::Small))
                    .child(div().min_w_0().flex_1().child(dropdown(
                        format!("settings-profile-{index}-icon"),
                        ProfileIcon::selector_label(profile.icon.as_ref()).to_owned(),
                        SettingsDropdown::ProfileIcon(index),
                    )));
                let visibility_handle = handle.clone();
                let profile_visibility = switch(
                    format!("settings-profile-{index}-visibility"),
                    (!profile.hidden).into(),
                )
                .label(if profile.hidden { "Hidden" } else { "Visible" })
                .full_width(true)
                .aria_label("Show profile in Profiles menu")
                .on_click(move |state, window, cx| {
                    visibility_handle
                        .update(cx, |this, cx| {
                            this.set_settings_toggle(
                                SettingsToggle::ProfileVisibility(index),
                                state.selected(),
                                window,
                                cx,
                            );
                        })
                        .ok();
                });
                let card = if profile.detected {
                    div()
                        .p_3()
                        .mb_2()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if profile_focused {
                            colors.border_focused
                        } else {
                            colors.border
                        })
                        .bg(if profile_focused {
                            colors.element_selected
                        } else {
                            colors.editor_background
                        })
                        .child(
                            h_flex()
                                .min_w_0()
                                .flex_1()
                                .gap_2()
                                .child(profile_icon_value.render(IconSize::Medium))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(colors.text)
                                                .child(profile.name.text.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child("Detected profile"),
                                        ),
                                ),
                        )
                        .child(profile_fields_grid([
                            profile_field("Shown in Profiles menu", profile_visibility, colors),
                            profile_field("Icon", profile_icon, colors),
                            profile_field("Light theme", profile_theme, colors),
                            profile_field("Dark theme", profile_dark_theme, colors),
                        ]))
                        .into_any_element()
                } else {
                    let remove_handle = handle.clone();
                    div()
                        .p_3()
                        .mb_2()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if profile_focused {
                            colors.border_focused
                        } else {
                            colors.border
                        })
                        .bg(if profile_focused {
                            colors.element_selected
                        } else {
                            colors.editor_background
                        })
                        .child(
                            h_flex()
                                .items_end()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .mb_1()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child("Profile name"),
                                        )
                                        .child(text_input(
                                            format!("settings-profile-{index}-name"),
                                            profile.name.clone(),
                                            SettingsInput::Configuration(
                                                ConfigTextField::ProfileName(index),
                                            ),
                                        )),
                                )
                                .child(
                                    IconButton::new(
                                        ("remove-settings-profile", index),
                                        IconName::Trash,
                                    )
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Custom(colors.icon))
                                    .selected_icon_color(Color::Custom(colors.icon))
                                    .toggle_state(
                                        editor.focused_control
                                            == Some(SettingsControl::RemoveProfile(index)),
                                    )
                                    .selected_style(ButtonStyle::OutlinedCustom(
                                        colors.border_focused,
                                    ))
                                    .tooltip(Tooltip::text("Remove profile"))
                                    .on_click(
                                        move |_, _, cx| {
                                            remove_handle
                                                .update(cx, |this, cx| {
                                                    if let Some(editor) =
                                                        this.settings_editor.as_mut()
                                                    {
                                                        editor.configuration.profiles.remove(index);
                                                        editor.configuration_dirty = true;
                                                        invalidate_controls_cache(editor);
                                                        cx.notify();
                                                    }
                                                })
                                                .ok();
                                        },
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .mt_2()
                                .grid()
                                .grid_cols(2)
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .mb_1()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child("Program"),
                                        )
                                        .child(text_input(
                                            format!("settings-profile-{index}-program"),
                                            profile.program.clone(),
                                            SettingsInput::Configuration(
                                                ConfigTextField::ProfileProgram(index),
                                            ),
                                        )),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .mb_1()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child("Arguments (comma separated)"),
                                        )
                                        .child(text_input(
                                            format!("settings-profile-{index}-arguments"),
                                            profile.arguments.clone(),
                                            SettingsInput::Configuration(
                                                ConfigTextField::ProfileArguments(index),
                                            ),
                                        )),
                                ),
                        )
                        .child(profile_fields_grid([
                            profile_field("Shown in Profiles menu", profile_visibility, colors),
                            profile_field("Icon", profile_icon, colors),
                            profile_field("Light theme", profile_theme, colors),
                            profile_field("Dark theme", profile_dark_theme, colors),
                        ]))
                        .into_any_element()
                };
                rows.push(
                    track_focus_scroll(div().w_full().child(card), editor, &profile_controls)
                        .into_any_element(),
                );
            }
            let add_handle = handle.clone();
            let add_focused = editor.focused_control == Some(SettingsControl::AddProfile);
            rows.push(
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("add-settings-profile", "Add profile")
                            .style(ButtonStyle::Outlined)
                            .color(Color::Custom(colors.text))
                            .selected_label_color(Color::Custom(colors.text))
                            .toggle_state(add_focused)
                            .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                            .on_click(move |_, window, cx| {
                                add_handle
                                    .update(cx, |this, cx| {
                                        if let Some(editor) = this.settings_editor.as_mut() {
                                            editor.profile_draft_scroll = ScrollHandle::new();
                                            editor.profile_draft =
                                                Some(settings_editor::ProfileForm {
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
                                            editor.message = None;
                                            invalidate_controls_cache(editor);
                                        }
                                        this.focus_settings_input(
                                            SettingsInput::ProfileDraft(ProfileDraftField::Name),
                                            window,
                                            cx,
                                        );
                                    })
                                    .ok();
                            }),
                    )
                    .into_any_element(),
            );
            div().children(rows).into_any_element()
        }
        SettingsPage::Themes => {
            let search = text_input(
                "settings-theme-extension-search".to_owned(),
                editor.theme_extension_query.clone(),
                SettingsInput::ThemeSearch,
            );
            let search_handle = handle.clone();
            let mut rows = vec![
                    div()
                        .mb_3()
                        .child(
                            div()
                                .mb_1()
                                .text_sm()
                                .child("Download themes from Zed extensions"),
                        )
                        .child(
                            div()
                                .mb_3()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(
                                    "Only declared theme JSON files are installed. Other extension features are ignored.",
                                )
                                .child(
                                    div().mt_1().child(
                                        ButtonLink::new(
                                            "Browse the Zed themes store",
                                            "https://zed.dev/extensions?filter=themes",
                                        )
                                        .label_size(LabelSize::Small),
                                    ),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(search))
                                .child(
                                    div()
                                        .id("search-theme-extensions")
                                        .h_9()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(
                                            if editor.focused_control
                                                == Some(SettingsControl::SearchThemes)
                                            {
                                                colors.border_focused
                                            } else {
                                                colors.border
                                            },
                                        )
                                        .when(
                                            editor.focused_control
                                                == Some(SettingsControl::SearchThemes),
                                            |button| button.bg(colors.element_selected),
                                        )
                                        .cursor_pointer()
                                        .text_color(colors.text)
                                        .hover(|style| style.bg(colors.element_hover))
                                        .on_click(move |_, window, cx| {
                                            search_handle
                                                .update(cx, |this, cx| {
                                                    this.fetch_theme_extensions(window, cx)
                                                })
                                                .ok();
                                        })
                                        .child(if editor.theme_extensions_loading {
                                            "Loading…"
                                        } else {
                                            "Search"
                                        }),
                                ),
                        )
                        .into_any_element(),
                ];
            if !editor.installed_theme_extensions.is_empty() {
                rows.push(
                    div()
                        .mt_2()
                        .mb_2()
                        .text_sm()
                        .child("Installed from Zed extensions")
                        .into_any_element(),
                );
                for installed in &editor.installed_theme_extensions {
                    let id = installed.id.clone();
                    let removing = editor
                        .theme_extension_downloading
                        .as_ref()
                        .is_some_and(|active| active.as_ref() == installed.id);
                    let disabled = editor.theme_extension_downloading.is_some();
                    let remove_handle = handle.clone();
                    let theme_names = installed.theme_names.join(", ");
                    let focused = editor.focused_control
                        == Some(SettingsControl::RemoveTheme(installed.id.clone()));
                    rows.push(
                        div()
                            .mb_2()
                            .p_3()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(if focused {
                                colors.element_selected
                            } else {
                                colors.editor_background
                            })
                            .child(
                                h_flex()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .child(div().text_sm().child(installed.id.clone()))
                                            .child(
                                                div()
                                                    .mt_1()
                                                    .text_xs()
                                                    .text_color(colors.text_muted)
                                                    .child(format!(
                                                        "{} theme file{}{}",
                                                        installed.file_count,
                                                        if installed.file_count == 1 {
                                                            ""
                                                        } else {
                                                            "s"
                                                        },
                                                        if theme_names.is_empty() {
                                                            String::new()
                                                        } else {
                                                            format!(" · {theme_names}")
                                                        }
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(format!("remove-theme-extension-{}", installed.id))
                                            .h_8()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .flex_none()
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(
                                                if editor.focused_control
                                                    == Some(SettingsControl::RemoveTheme(
                                                        installed.id.clone(),
                                                    ))
                                                {
                                                    colors.border_focused
                                                } else {
                                                    colors.border
                                                },
                                            )
                                            .when(!disabled, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(colors.element_hover))
                                                    .on_click(move |_, window, cx| {
                                                        remove_handle
                                                            .update(cx, |this, cx| {
                                                                this.remove_theme_extension(
                                                                    id.clone(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            })
                                                            .ok();
                                                    })
                                            })
                                            .child(if removing { "Removing…" } else { "Remove" }),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
            }
            if editor.theme_extensions.is_empty() && !editor.theme_extensions_loading {
                rows.push(
                    div()
                        .py_6()
                        .text_center()
                        .text_color(colors.text_muted)
                        .child(if editor.theme_extensions_searched {
                            "No matching theme extensions found."
                        } else {
                            "Enter a theme name and select Search."
                        })
                        .into_any_element(),
                );
            }
            for extension in &editor.theme_extensions {
                let id = extension.id.clone();
                let downloading = editor
                    .theme_extension_downloading
                    .as_ref()
                    .is_some_and(|active| active == &id);
                let already_installed = editor
                    .installed_theme_extensions
                    .iter()
                    .any(|installed| installed.id == extension.id.as_ref());
                let disabled = editor.theme_extension_downloading.is_some() || already_installed;
                let install_handle = handle.clone();
                let focused = editor.focused_control
                    == Some(SettingsControl::InstallTheme(extension.id.clone()));
                let description = extension
                    .description
                    .clone()
                    .unwrap_or_else(|| "Theme extension for Zed".to_owned());
                let author = if extension.authors.is_empty() {
                    String::new()
                } else {
                    format!(" by {}", extension.authors.join(", "))
                };
                rows.push(
                    div()
                        .mb_2()
                        .p_3()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if focused {
                            colors.border_focused
                        } else {
                            colors.border
                        })
                        .bg(if focused {
                            colors.element_selected
                        } else {
                            colors.editor_background
                        })
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .child(format!("{}{}", extension.name, author)),
                                        )
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child(description),
                                        )
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_xs()
                                                .text_color(colors.text_muted)
                                                .child(format!(
                                                    "{} downloads · version {}",
                                                    extension.download_count, extension.version
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(format!("install-theme-extension-{}", extension.id))
                                        .h_8()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .flex_none()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(
                                            if editor.focused_control
                                                == Some(SettingsControl::InstallTheme(
                                                    extension.id.clone(),
                                                ))
                                            {
                                                colors.border_focused
                                            } else {
                                                colors.border
                                            },
                                        )
                                        .when(!disabled, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(colors.element_hover))
                                                .on_click(move |_, window, cx| {
                                                    install_handle
                                                        .update(cx, |this, cx| {
                                                            this.download_theme_extension(
                                                                id.clone(),
                                                                window,
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                })
                                        })
                                        .child(if downloading {
                                            "Installing…"
                                        } else if already_installed {
                                            "Installed"
                                        } else {
                                            "Install"
                                        }),
                                ),
                        )
                        .into_any_element(),
                );
            }
            div().children(rows).into_any_element()
        }
        SettingsPage::Keymap => {
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
                move |zetta, range, window, cx| {
                    compute_keymap_sticky_candidates(zetta, range, window, cx)
                },
                move |zetta, candidate, window, cx| {
                    render_keymap_sticky_candidate(zetta, candidate, window, cx)
                },
            ));
            let keymap_scroll = editor.keymap_scroll.0.borrow().base_handle.clone();

            div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(
                        div()
                            .flex_none()
                            .mb_3()
                            .text_sm()
                            .child("Type an accelerator in the field, or click Record to capture one from your keyboard.")
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(colors.text_muted)
                                    .child("Recording opens a confirmation dialog: press Return to use the captured shortcut or Esc to cancel."),
                            ),
                    )
                    .child(
                        div().flex_none().mb_3().child(text_input(
                            "settings-keymap-search".to_owned(),
                            editor.keymap_search.clone(),
                            SettingsInput::KeymapSearch,
                        )),
                    )
                    .when(no_results, |content| {
                        content.child(
                            div()
                                .flex_none()
                                .mb_3()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child("No bindings match your search."),
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
        SettingsPage::PaneTemplates => render_pane_templates_page(editor, colors, handle),
        SettingsPage::Projects => render_projects_page(editor, colors, handle, opacity_slider),
    }
}

#[cfg(test)]
#[path = "../tests/settings_view/pages.rs"]
mod tests;
