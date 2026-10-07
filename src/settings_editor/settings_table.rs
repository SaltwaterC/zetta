//! Every scalar setting the Configuration page edits, as one table.
//!
//! A setting used to be spelled out in about sixteen places: the form field,
//! how `load` read it, how `to_json` wrote and checked it, the defaults list
//! that strips it, a variant in each of the toggle, dropdown, stepper and
//! text-field enums with a getter and a setter arm, a stepper arm, a tab stop,
//! and a page row. [`ConfigSetting`] names a setting once, and its
//! [`SettingSpec`] says where it lives in the file, what kind of value it is
//! and what it defaults to. Loading, saving, default stripping, per-field
//! validation, the generic switch/dropdown/stepper/text handling, the page's
//! controls and its tab order all come from the table, so a new setting is its
//! `Config` field, its form field, one entry here, and its label.
//!
//! What is not a scalar — profiles, pane templates — stays with the form, and
//! a scalar with a bespoke editor (a theme, the font, the tab icon, the
//! opacity slider) is [`SettingKind::Custom`]: it still loads, saves and strips
//! through the table, and only its control is drawn by hand.

use super::*;
use crate::config::{RemoteSessionProtocol, SessionRetention};

/// One scalar setting of the user configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ConfigSetting {
    DefaultProfile,
    NewTabProfile,
    DefaultTabIcon,
    WorkingDirectory,
    WorkingDirectoryScope,
    LightTheme,
    DarkTheme,
    FontSize,
    FontFamily,
    ScrollHistory,
    MouseClipboard,
    InactivePaneOpacity,
    CompactMode,
    ShowPaneSize,
    ShowTitleBarLabels,
    ShowTitleBarButtons,
    #[cfg(target_os = "macos")]
    ShowTitleBarMenus,
    PaneControlsPosition,
    ShowPaneControls,
    SessionRetention,
    SessionRingBytes,
    #[cfg(feature = "session-persistence")]
    DiskRecipients,
    #[cfg(feature = "session-persistence")]
    IdentityFile,
    #[cfg(feature = "session-persistence")]
    AutoProtect,
    RemoteProtocol,
    RemoteKeepAlive,
    ForwardAgent,
    #[cfg(feature = "http-server")]
    HttpServerPort,
    #[cfg(feature = "tftp-server")]
    TftpServerPort,
}

/// Every setting this build has, in no meaningful order: what the page draws
/// and in which order is `settings_ui::configuration_page`'s business.
pub(crate) const ALL_SETTINGS: &[ConfigSetting] = &[
    ConfigSetting::DefaultProfile,
    ConfigSetting::NewTabProfile,
    ConfigSetting::DefaultTabIcon,
    ConfigSetting::WorkingDirectory,
    ConfigSetting::WorkingDirectoryScope,
    ConfigSetting::LightTheme,
    ConfigSetting::DarkTheme,
    ConfigSetting::FontSize,
    ConfigSetting::FontFamily,
    ConfigSetting::ScrollHistory,
    ConfigSetting::MouseClipboard,
    ConfigSetting::InactivePaneOpacity,
    ConfigSetting::CompactMode,
    ConfigSetting::ShowPaneSize,
    ConfigSetting::ShowTitleBarLabels,
    ConfigSetting::ShowTitleBarButtons,
    #[cfg(target_os = "macos")]
    ConfigSetting::ShowTitleBarMenus,
    ConfigSetting::PaneControlsPosition,
    ConfigSetting::ShowPaneControls,
    ConfigSetting::SessionRetention,
    ConfigSetting::SessionRingBytes,
    #[cfg(feature = "session-persistence")]
    ConfigSetting::DiskRecipients,
    #[cfg(feature = "session-persistence")]
    ConfigSetting::IdentityFile,
    #[cfg(feature = "session-persistence")]
    ConfigSetting::AutoProtect,
    ConfigSetting::RemoteProtocol,
    ConfigSetting::RemoteKeepAlive,
    ConfigSetting::ForwardAgent,
    #[cfg(feature = "http-server")]
    ConfigSetting::HttpServerPort,
    #[cfg(feature = "tftp-server")]
    ConfigSetting::TftpServerPort,
];

/// Where a setting lives in the file, and what kind of value it is.
pub(crate) struct SettingSpec {
    /// The key path from the file's root: `["sessions", "remote", "protocol"]`.
    pub(crate) key: &'static [&'static str],
    pub(crate) kind: SettingKind,
}

pub(crate) enum SettingKind {
    Switch(SwitchSpec),
    Choice(ChoiceSpec),
    Number(NumberSpec),
    Text(TextSpec),
    Custom(CustomSpec),
}

/// A boolean, drawn as a switch.
pub(crate) struct SwitchSpec {
    get: fn(&ConfigurationForm) -> bool,
    set: fn(&mut ConfigurationForm, bool),
    default: bool,
    /// The file stores the opposite of what the switch says: a `hide_…` key
    /// behind a "Show …" switch.
    pub(crate) shown_inverted: bool,
}

/// One of a fixed set of values, drawn as a dropdown.
pub(crate) struct ChoiceSpec {
    /// Each value as the file spells it, and as the dropdown labels it.
    pub(crate) options: &'static [(&'static str, &'static str)],
    get: fn(&ConfigurationForm) -> &'static str,
    set: fn(&mut ConfigurationForm, &str),
    default: &'static str,
}

/// The text field a number or text setting is typed into.
#[derive(Clone, Copy)]
pub(crate) struct TextAccess {
    get: fn(&ConfigurationForm) -> &TextField,
    get_mut: fn(&mut ConfigurationForm) -> &mut TextField,
}

impl TextAccess {
    pub(crate) fn field(self, form: &ConfigurationForm) -> &TextField {
        (self.get)(form)
    }
}

/// A number typed into a field between `−` and `+` steppers.
pub(crate) struct NumberSpec {
    pub(crate) text: TextAccess,
    /// What the number is called in an error: "Font size must be …".
    noun: &'static str,
    /// What it counts, if anything: "bytes", "milliseconds".
    unit: Option<&'static str>,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) integer: bool,
    pub(crate) step: Step,
    /// What an empty field means.
    pub(crate) empty: Empty,
    /// A word standing for one value, such as `Max` for the most scrollback.
    pub(crate) sentinel: Option<(&'static str, f64)>,
    /// The value the file means by leaving the key out. `None` when leaving it
    /// out means "unset" rather than any one number.
    default: Option<f64>,
    /// Where stepping an empty field starts from.
    pub(crate) start: Start,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Step {
    By(f64),
    /// Scrollback's steps, which grow with the value.
    Accelerating,
}

/// What an empty number field means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Empty {
    /// Nothing: it has to hold a number.
    Invalid,
    /// Leave the key out of the file; something else decides the value.
    Unset(Placeholder),
    /// Write `null`.
    Null(Placeholder),
}

/// What an empty field shows instead of nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Placeholder {
    /// The terminal font size the theme gives, which only the running dialog
    /// knows.
    ThemeFontSize,
    Text(&'static str),
}

/// Where stepping an empty number field starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Start {
    Value(f64),
    ThemeFontSize,
}

/// Free text.
pub(crate) struct TextSpec {
    pub(crate) text: TextAccess,
    shape: TextShape,
}

#[derive(Clone, Copy)]
enum TextShape {
    /// A string, defaulting to `default`; any of `same_as_default` is the
    /// default spelled another way.
    Plain {
        default: &'static str,
        same_as_default: &'static [&'static str],
    },
    /// A string, or `null` while the field is empty.
    #[cfg(feature = "session-persistence")]
    NullWhenEmpty,
    /// A comma-separated field written as a list of strings.
    #[cfg(feature = "session-persistence")]
    CommaList,
}

/// A setting with an editor of its own. It loads, saves and strips through
/// these three functions; only its control is drawn by hand.
pub(crate) struct CustomSpec {
    encode: fn(&ConfigurationForm) -> Value,
    decode: fn(&mut ConfigurationForm, Option<&Value>, &Config),
    default: fn(&ConfigurationForm) -> Value,
}

macro_rules! switch {
    ($field:ident, default $default:expr) => {
        switch!($field, default $default, shown_inverted false)
    };
    ($field:ident, default $default:expr, shown_inverted $inverted:expr) => {
        SettingKind::Switch(SwitchSpec {
            get: |form| form.$field,
            set: |form, value| form.$field = value,
            default: $default,
            shown_inverted: $inverted,
        })
    };
}

macro_rules! text_access {
    ($field:ident) => {
        TextAccess {
            get: |form| &form.$field,
            get_mut: |form| &mut form.$field,
        }
    };
}

/// The two retention options every build has, and disk where it is built in.
#[cfg(feature = "session-persistence")]
const RETENTION_OPTIONS: &[(&str, &str)] =
    &[("none", "None"), ("memory", "Memory"), ("disk", "Disk")];
#[cfg(not(feature = "session-persistence"))]
const RETENTION_OPTIONS: &[(&str, &str)] = &[("none", "None"), ("memory", "Memory")];

impl ConfigSetting {
    pub(crate) fn spec(self) -> SettingSpec {
        use SettingKind as K;
        let (key, kind): (&'static [&'static str], SettingKind) = match self {
            Self::DefaultProfile => (
                &["default_profile"],
                K::Custom(CustomSpec {
                    encode: |form| json!(form.default_profile),
                    // The name the file's choice resolved to, spelled as the
                    // profile spells it rather than as the file did.
                    decode: |form, _, config| {
                        form.default_profile = config.profiles[config.default_profile].name.clone();
                    },
                    // The first profile is the one used when none is named, and
                    // the file matches profile names regardless of case.
                    default: |form| match form.profiles.first() {
                        Some(first)
                            if first.name.text.eq_ignore_ascii_case(&form.default_profile) =>
                        {
                            json!(form.default_profile)
                        }
                        Some(first) => json!(first.name.text),
                        None => Value::Null,
                    },
                }),
            ),
            Self::NewTabProfile => (
                &["new_tab_profile"],
                K::Choice(ChoiceSpec {
                    options: &[("default", "Default"), ("inherit", "Inherit")],
                    get: |form| form.new_tab_profile.as_str(),
                    set: |form, value| {
                        if let Ok(value) = NewTabProfile::parse(value) {
                            form.new_tab_profile = value;
                        }
                    },
                    default: "default",
                }),
            ),
            Self::DefaultTabIcon => (
                &["default_tab_icon"],
                K::Custom(CustomSpec {
                    encode: |form| {
                        form.default_tab_icon.map_or(Value::Null, |icon| {
                            let name: &'static str = icon.into();
                            json!(name)
                        })
                    },
                    decode: |form, value, _| {
                        form.default_tab_icon = match value {
                            None => Some(IconName::Terminal),
                            Some(Value::Null) => None,
                            Some(value) => value
                                .as_str()
                                .and_then(|name| name.parse().ok())
                                .or(Some(IconName::Terminal)),
                        };
                    },
                    default: |_| json!("terminal"),
                }),
            ),
            Self::WorkingDirectory => (
                &["working_directory"],
                K::Text(TextSpec {
                    text: text_access!(working_directory),
                    shape: TextShape::Plain {
                        default: "~",
                        same_as_default: &["~/"],
                    },
                }),
            ),
            Self::WorkingDirectoryScope => (
                &["working_directory_scope"],
                K::Choice(ChoiceSpec {
                    options: &[("none", "None"), ("pane", "Pane"), ("tab", "Tab")],
                    get: |form| form.working_directory_scope.as_str(),
                    set: |form, value| {
                        if let Ok(value) = WorkingDirectoryScope::parse(value) {
                            form.working_directory_scope = value;
                        }
                    },
                    default: "tab",
                }),
            ),
            Self::LightTheme => (
                &["theme"],
                K::Custom(CustomSpec {
                    encode: |form| json!(form.theme),
                    decode: |form, value, _| {
                        form.theme = value
                            .and_then(Value::as_str)
                            .unwrap_or(crate::ZETTA_DEFAULT_THEME)
                            .to_owned();
                    },
                    default: |_| json!(crate::ZETTA_DEFAULT_THEME),
                }),
            ),
            Self::DarkTheme => (
                &["dark_theme"],
                K::Custom(CustomSpec {
                    encode: |form| json!(form.dark_theme),
                    decode: |form, value, _| {
                        form.dark_theme = value
                            .and_then(Value::as_str)
                            .unwrap_or(crate::ZETTA_DEFAULT_DARK_THEME)
                            .to_owned();
                    },
                    default: |_| json!(crate::ZETTA_DEFAULT_DARK_THEME),
                }),
            ),
            Self::FontSize => (
                &["terminal_font_size"],
                K::Number(NumberSpec {
                    text: text_access!(terminal_font_size),
                    noun: "Font size",
                    unit: None,
                    min: f64::from(crate::config::MIN_TERMINAL_FONT_SIZE),
                    max: f64::from(crate::config::MAX_TERMINAL_FONT_SIZE),
                    integer: false,
                    step: Step::By(1.),
                    // Unset: the theme's buffer size applies, and follows the
                    // theme. Writing any number back would pin one.
                    empty: Empty::Unset(Placeholder::ThemeFontSize),
                    sentinel: None,
                    default: None,
                    start: Start::ThemeFontSize,
                }),
            ),
            Self::FontFamily => (
                &["terminal_font_family"],
                K::Custom(CustomSpec {
                    encode: |form| json!(form.terminal_font_family),
                    decode: |form, value, _| {
                        form.terminal_font_family = value
                            .and_then(Value::as_str)
                            .filter(|family| !family.trim().is_empty())
                            .unwrap_or(crate::config::DEFAULT_TERMINAL_FONT_FAMILY)
                            .to_owned();
                    },
                    default: |_| json!(crate::config::DEFAULT_TERMINAL_FONT_FAMILY),
                }),
            ),
            Self::ScrollHistory => (
                &["max_scroll_history_lines"],
                K::Number(NumberSpec {
                    text: text_access!(max_scroll_history_lines),
                    noun: "Scrollback history",
                    unit: Some("lines"),
                    min: 0.,
                    max: terminal::MAX_SCROLL_HISTORY_LINES as f64,
                    integer: true,
                    step: Step::Accelerating,
                    empty: Empty::Invalid,
                    sentinel: Some(("Max", terminal::MAX_SCROLL_HISTORY_LINES as f64)),
                    default: Some(terminal::MAX_SCROLL_HISTORY_LINES as f64),
                    start: Start::Value(0.),
                }),
            ),
            Self::MouseClipboard => (&["mouse_clipboard"], switch!(mouse_clipboard, default true)),
            Self::InactivePaneOpacity => (
                &["inactive_pane_opacity"],
                K::Custom(CustomSpec {
                    encode: |form| json!(two_decimals(f64::from(form.inactive_pane_opacity))),
                    decode: |form, value, _| {
                        form.inactive_pane_opacity = value
                            .and_then(Value::as_f64)
                            .map_or(crate::config::DEFAULT_INACTIVE_PANE_OPACITY, |opacity| {
                                opacity as f32
                            });
                    },
                    default: |_| {
                        json!(two_decimals(f64::from(
                            crate::config::DEFAULT_INACTIVE_PANE_OPACITY
                        )))
                    },
                }),
            ),
            Self::CompactMode => (&["compact_mode"], switch!(compact_mode, default false)),
            Self::ShowPaneSize => (
                &["hide_pane_size"],
                switch!(hide_pane_size, default true, shown_inverted true),
            ),
            Self::ShowTitleBarLabels => (
                &["hide_title_bar_labels"],
                switch!(hide_title_bar_labels, default false, shown_inverted true),
            ),
            Self::ShowTitleBarButtons => (
                &["hide_title_bar_buttons"],
                switch!(hide_title_bar_buttons, default false, shown_inverted true),
            ),
            #[cfg(target_os = "macos")]
            Self::ShowTitleBarMenus => (
                &["hide_title_bar_menus"],
                switch!(hide_title_bar_menus, default true, shown_inverted true),
            ),
            Self::PaneControlsPosition => (
                &["pane_controls_position"],
                K::Choice(ChoiceSpec {
                    options: &[("right", "Right"), ("left", "Left")],
                    get: |form| form.pane_controls_position.as_str(),
                    set: |form, value| {
                        if let Ok(value) = PaneControlsPosition::parse(value) {
                            form.pane_controls_position = value;
                        }
                    },
                    default: "right",
                }),
            ),
            Self::ShowPaneControls => (
                &["pane_controls_hidden_by_default"],
                switch!(pane_controls_hidden_by_default, default false, shown_inverted true),
            ),
            Self::SessionRetention => (
                &["sessions", "retention"],
                K::Choice(ChoiceSpec {
                    options: RETENTION_OPTIONS,
                    get: |form| form.session_retention.as_str(),
                    set: |form, value| {
                        if let Ok(value) = SessionRetention::parse(value) {
                            form.session_retention = value;
                        }
                    },
                    default: "memory",
                }),
            ),
            Self::SessionRingBytes => (
                &["sessions", "ring_bytes"],
                K::Number(NumberSpec {
                    text: text_access!(session_ring_bytes),
                    noun: "Retained screen size",
                    unit: Some("bytes"),
                    min: 4096.,
                    max: crate::config::MAX_SESSION_RING_BYTES as f64,
                    integer: true,
                    step: Step::By(4096.),
                    empty: Empty::Invalid,
                    sentinel: None,
                    default: Some(crate::config::DEFAULT_SESSION_RING_BYTES as f64),
                    start: Start::Value(crate::config::DEFAULT_SESSION_RING_BYTES as f64),
                }),
            ),
            #[cfg(feature = "session-persistence")]
            Self::DiskRecipients => (
                &["sessions", "persistence", "recipients"],
                K::Text(TextSpec {
                    text: text_access!(session_persistence_recipients),
                    shape: TextShape::CommaList,
                }),
            ),
            #[cfg(feature = "session-persistence")]
            Self::IdentityFile => (
                &["sessions", "persistence", "identity"],
                K::Text(TextSpec {
                    text: text_access!(session_persistence_identity),
                    shape: TextShape::NullWhenEmpty,
                }),
            ),
            #[cfg(feature = "session-persistence")]
            Self::AutoProtect => (
                &["sessions", "persistence", "auto_protect"],
                switch!(session_persistence_auto_protect, default false),
            ),
            Self::RemoteProtocol => (
                &["sessions", "remote", "protocol"],
                K::Choice(ChoiceSpec {
                    options: &[("ssh", "SSH"), ("zosh", "Zosh")],
                    get: |form| form.remote_session_protocol.name(),
                    set: |form, value| {
                        if let Ok(value) = RemoteSessionProtocol::parse(value) {
                            form.remote_session_protocol = value;
                        }
                    },
                    default: "ssh",
                }),
            ),
            Self::RemoteKeepAlive => (
                &["sessions", "remote", "keep_alive_ms"],
                K::Number(NumberSpec {
                    text: text_access!(remote_session_keep_alive),
                    noun: "Remote keep-alive",
                    unit: Some("milliseconds"),
                    min: crate::config::REMOTE_KEEP_ALIVE_MIN_MS as f64,
                    max: crate::config::REMOTE_KEEP_ALIVE_MAX_MS as f64,
                    integer: true,
                    step: Step::By(10.),
                    empty: Empty::Null(Placeholder::Text("Mosh default")),
                    sentinel: None,
                    default: None,
                    // An empty field means Mosh's own heartbeat, so stepping
                    // up from it starts at the interval the protocol suggests
                    // rather than at the smallest it allows.
                    start: Start::Value(crate::config::REMOTE_KEEP_ALIVE_DEFAULT_MS as f64),
                }),
            ),
            Self::ForwardAgent => (
                &["sessions", "remote", "forward_agent"],
                switch!(remote_session_forward_agent, default false),
            ),
            #[cfg(feature = "http-server")]
            Self::HttpServerPort => (
                &["http_server_port"],
                port(
                    text_access!(http_server_port),
                    "HTTP server port",
                    crate::config::DEFAULT_HTTP_PORT,
                ),
            ),
            #[cfg(feature = "tftp-server")]
            Self::TftpServerPort => (
                &["tftp_server_port"],
                port(
                    text_access!(tftp_server_port),
                    "TFTP server port",
                    crate::config::DEFAULT_TFTP_SERVER_PORT,
                ),
            ),
        };
        SettingSpec { key, kind }
    }

    /// The text field the setting is typed into, for a number or text setting.
    pub(crate) fn text_mut(self, form: &mut ConfigurationForm) -> Option<&mut TextField> {
        match self.spec().kind {
            SettingKind::Number(NumberSpec { text, .. })
            | SettingKind::Text(TextSpec { text, .. }) => Some((text.get_mut)(form)),
            _ => None,
        }
    }

    /// A switch's state as the switch shows it.
    pub(crate) fn switch_shown(self, form: &ConfigurationForm) -> Option<bool> {
        match self.spec().kind {
            SettingKind::Switch(spec) => Some((spec.get)(form) != spec.shown_inverted),
            _ => None,
        }
    }

    pub(crate) fn set_switch_shown(self, form: &mut ConfigurationForm, shown: bool) {
        if let SettingKind::Switch(spec) = self.spec().kind {
            (spec.set)(form, shown != spec.shown_inverted);
        }
    }

    /// A choice's current option, as its label.
    pub(crate) fn choice_label(self, form: &ConfigurationForm) -> Option<&'static str> {
        let SettingKind::Choice(spec) = self.spec().kind else {
            return None;
        };
        let value = (spec.get)(form);
        spec.options
            .iter()
            .find(|(file, _)| *file == value)
            .map(|(_, label)| *label)
    }

    /// Sets a choice from the option at `index` of its options.
    pub(crate) fn set_choice(self, form: &mut ConfigurationForm, index: usize) {
        if let SettingKind::Choice(spec) = self.spec().kind
            && let Some((value, _)) = spec.options.get(index)
        {
            (spec.set)(form, value);
        }
    }

    /// Steps a number by `direction` steps from its current value, or from its
    /// starting point while it is empty or unreadable.
    pub(crate) fn step_number(
        self,
        form: &mut ConfigurationForm,
        direction: i32,
        theme_font_size: f32,
    ) {
        let SettingKind::Number(spec) = self.spec().kind else {
            return;
        };
        let field = (spec.text.get_mut)(form);
        let start = match spec.start {
            Start::Value(value) => value,
            Start::ThemeFontSize => f64::from(theme_font_size),
        };
        // Stepping is clamped after the step, not before, so a number typed
        // out of range steps onto the end it is beyond: 200 and − is the
        // largest font size.
        let current = spec.read_unchecked(&field.text).unwrap_or(start);
        let next = match spec.step {
            Step::By(step) => current + step * f64::from(direction),
            Step::Accelerating => {
                adjusted_scroll_history(current as u64, direction, spec.max as u64) as f64
            }
        }
        .clamp(spec.min, spec.max);
        *field = TextField::new(spec.format(next));
    }

    /// What the setting's value is in the file, or `None` to leave the key out.
    /// An error says, in the page's words, why the value cannot be saved.
    pub(crate) fn encode(self, form: &ConfigurationForm) -> Result<Option<Value>, String> {
        match self.spec().kind {
            SettingKind::Switch(spec) => Ok(Some(json!((spec.get)(form)))),
            SettingKind::Choice(spec) => Ok(Some(json!((spec.get)(form)))),
            SettingKind::Number(spec) => spec.encode(&(spec.text.get)(form).text),
            SettingKind::Text(spec) => Ok(Some(spec.encode(&(spec.text.get)(form).text))),
            SettingKind::Custom(spec) => Ok(Some((spec.encode)(form))),
        }
    }

    /// Fills the form from what the file holds, `None` when the key is absent.
    pub(crate) fn decode(
        self,
        form: &mut ConfigurationForm,
        value: Option<&Value>,
        config: &Config,
    ) {
        match self.spec().kind {
            SettingKind::Switch(spec) => {
                (spec.set)(form, value.and_then(Value::as_bool).unwrap_or(spec.default));
            }
            SettingKind::Choice(spec) => {
                (spec.set)(form, value.and_then(Value::as_str).unwrap_or(spec.default));
            }
            SettingKind::Number(spec) => {
                let number = value.and_then(Value::as_f64).or(spec.default);
                *(spec.text.get_mut)(form) =
                    TextField::new(number.map(|number| spec.format(number)).unwrap_or_default());
            }
            SettingKind::Text(spec) => {
                *(spec.text.get_mut)(form) = TextField::new(spec.decode(value));
            }
            SettingKind::Custom(spec) => (spec.decode)(form, value, config),
        }
    }

    /// The value the file means by leaving the key out, which a saved value
    /// equal to is left out again. `None` when there is no such value.
    pub(crate) fn default_value(self, form: &ConfigurationForm) -> Option<Value> {
        match self.spec().kind {
            SettingKind::Switch(spec) => Some(json!(spec.default)),
            SettingKind::Choice(spec) => Some(json!(spec.default)),
            SettingKind::Number(spec) => match (spec.default, spec.empty) {
                (Some(value), _) => Some(spec.json(value)),
                // Empty is written as `null`, which is what leaving it out
                // means too.
                (None, Empty::Null(_)) => Some(Value::Null),
                (None, Empty::Invalid | Empty::Unset(_)) => None,
            },
            SettingKind::Text(spec) => Some(spec.encode(&spec.default_text())),
            SettingKind::Custom(spec) => Some((spec.default)(form)),
        }
    }

    /// Writes the setting into `root`: its value at its key, or no key at all
    /// when the value is unset or the default.
    pub(crate) fn write(
        self,
        form: &ConfigurationForm,
        root: &mut Map<String, Value>,
    ) -> Result<(), String> {
        let key = self.spec().key;
        match self.encode(form)? {
            Some(value) if Some(&value) != self.default_value(form).as_ref() => {
                set_path(root, key, value);
            }
            _ => remove_path(root, key),
        }
        Ok(())
    }
}

impl NumberSpec {
    /// The number a field holds: `Ok(None)` for an empty field, an error for
    /// anything that is not a number this setting takes.
    fn read(&self, text: &str) -> Result<Option<f64>, String> {
        let text = text.trim();
        if text.is_empty() {
            return match self.empty {
                Empty::Invalid => Err(self.message()),
                Empty::Unset(_) | Empty::Null(_) => Ok(None),
            };
        }
        if let Some((word, value)) = self.sentinel
            && text.eq_ignore_ascii_case(word)
        {
            return Ok(Some(value));
        }
        text.parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && (self.min..=self.max).contains(number))
            .filter(|number| !self.integer || number.fract() == 0.)
            .map(Some)
            .ok_or_else(|| self.message())
    }

    /// Any number the field holds, or its sentinel, whether or not it is in
    /// range.
    fn read_unchecked(&self, text: &str) -> Option<f64> {
        let text = text.trim();
        match self.sentinel {
            Some((word, value)) if text.eq_ignore_ascii_case(word) => Some(value),
            _ => text.parse::<f64>().ok().filter(|number| number.is_finite()),
        }
    }

    fn encode(&self, text: &str) -> Result<Option<Value>, String> {
        Ok(match self.read(text)? {
            Some(number) => Some(self.json(number)),
            None => match self.empty {
                Empty::Null(_) => Some(Value::Null),
                Empty::Invalid | Empty::Unset(_) => None,
            },
        })
    }

    fn json(&self, number: f64) -> Value {
        if self.integer {
            json!(number as u64)
        } else {
            json!(number)
        }
    }

    /// How the field shows `number`: the sentinel word where it stands for it.
    fn format(&self, number: f64) -> String {
        match self.sentinel {
            Some((word, value)) if number == value => word.to_owned(),
            _ if self.integer => (number as u64).to_string(),
            _ => number.to_string(),
        }
    }

    /// The field's placeholder while it is empty, when empty means something.
    pub(crate) fn placeholder(&self) -> Option<Placeholder> {
        match self.empty {
            Empty::Invalid => None,
            Empty::Unset(placeholder) | Empty::Null(placeholder) => Some(placeholder),
        }
    }

    fn message(&self) -> String {
        let kind = if self.integer {
            "a whole number"
        } else {
            "a number"
        };
        let of = self
            .unit
            .map(|unit| format!(" of {unit}"))
            .unwrap_or_default();
        let sentinel = self
            .sentinel
            .map(|(word, _)| format!(", or {word}"))
            .unwrap_or_default();
        let empty = match self.empty {
            Empty::Invalid => "",
            Empty::Unset(_) => ", or empty for the default",
            Empty::Null(_) => ", or empty for none",
        };
        format!(
            "{} must be {kind}{of} from {} through {}{sentinel}{empty}",
            self.noun, self.min, self.max
        )
    }
}

impl TextSpec {
    fn encode(&self, text: &str) -> Value {
        match self.shape {
            TextShape::Plain {
                default,
                same_as_default,
            } => {
                let trimmed = text.trim();
                if same_as_default.contains(&trimmed) {
                    json!(default)
                } else {
                    json!(text)
                }
            }
            #[cfg(feature = "session-persistence")]
            TextShape::NullWhenEmpty => {
                let text = text.trim();
                if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                }
            }
            #[cfg(feature = "session-persistence")]
            TextShape::CommaList => Value::Array(
                text.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| json!(item))
                    .collect(),
            ),
        }
    }

    fn decode(&self, value: Option<&Value>) -> String {
        match (self.shape, value) {
            #[cfg(feature = "session-persistence")]
            (TextShape::CommaList, Some(Value::Array(items))) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            (_, Some(Value::String(text))) => text.clone(),
            _ => self.default_text(),
        }
    }

    fn default_text(&self) -> String {
        match self.shape {
            TextShape::Plain { default, .. } => default.to_owned(),
            #[cfg(feature = "session-persistence")]
            TextShape::NullWhenEmpty | TextShape::CommaList => String::new(),
        }
    }
}

/// A TCP or UDP port, from 1 through 65535.
#[cfg(servers_enabled)]
fn port(text: TextAccess, noun: &'static str, default: u16) -> SettingKind {
    SettingKind::Number(NumberSpec {
        text,
        noun,
        unit: None,
        min: 1.,
        max: f64::from(u16::MAX),
        integer: true,
        step: Step::By(1.),
        empty: Empty::Invalid,
        sentinel: None,
        default: Some(f64::from(default)),
        start: Start::Value(f64::from(default)),
    })
}

/// Opacity is written to two decimals, so a slider position does not leave a
/// float's tail in the file.
fn two_decimals(value: f64) -> f64 {
    (value * 100.).round() / 100.
}

/// Scrollback's stepper: steps that grow with the value, so the whole range
/// is reachable without holding the button for minutes.
pub(crate) fn adjusted_scroll_history(current: u64, direction: i32, maximum: u64) -> u64 {
    let step_basis = if direction < 0 {
        current.saturating_sub(1)
    } else {
        current
    };
    let step = match step_basis {
        0..100_000 => 1_000,
        100_000..1_000_000 => 100_000,
        1_000_000..10_000_000 => 1_000_000,
        10_000_000..100_000_000 => 10_000_000,
        _ => 100_000_000,
    };
    if direction < 0 {
        current.saturating_sub(step)
    } else {
        current.saturating_add(step).min(maximum)
    }
}

/// The value at `key` under `root`.
pub(crate) fn get_path<'a>(root: &'a Map<String, Value>, key: &[&str]) -> Option<&'a Value> {
    let (last, parents) = key.split_last()?;
    let mut object = root;
    for parent in parents {
        object = object.get(*parent)?.as_object()?;
    }
    object.get(*last)
}

/// Puts `value` at `key`, creating the objects on the way.
fn set_path(root: &mut Map<String, Value>, key: &[&str], value: Value) {
    let Some((last, parents)) = key.split_last() else {
        return;
    };
    let mut object = root;
    for parent in parents {
        let entry = object
            .entry((*parent).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        object = entry.as_object_mut().expect("just made an object");
    }
    object.insert((*last).to_owned(), value);
}

/// Removes `key`, and any object on the way that this leaves empty — but not
/// one that still holds a key Zetta does not know, which the file keeps.
fn remove_path(root: &mut Map<String, Value>, key: &[&str]) {
    let Some((first, rest)) = key.split_first() else {
        return;
    };
    if rest.is_empty() {
        root.remove(*first);
        return;
    }
    let Some(child) = root.get_mut(*first).and_then(Value::as_object_mut) else {
        return;
    };
    remove_path(child, rest);
    if child.is_empty() {
        root.remove(*first);
    }
}

#[cfg(test)]
#[path = "../tests/settings_editor/settings_table.rs"]
mod tests;
