//! What the Configuration page shows, top to bottom: its sections, the rows in
//! each, and what each row is labelled and described as.
//!
//! The page used to be drawn by one list and tabbed through by another, kept in
//! step by hand, and the two drifted: a comment in the tab order recorded the
//! time a whole section was tabbed after the rows it was drawn before, so that
//! focusing one of its fields scrolled it off screen. Both are built from
//! [`configuration_layout`] now. The view renders each [`ConfigSetting`]'s
//! control; everything else about the row — whether it is shown at all, what
//! it says, and which control it is in the tab order — is decided here.

use super::*;
use crate::settings_editor::SettingKind;

/// A section of the Configuration page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigurationSection {
    NewTabs,
    Terminal,
    Window,
    BackgroundSessions,
    RemoteSessions,
    #[cfg(servers_enabled)]
    NetworkServices,
    Profiles,
}

impl ConfigurationSection {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::NewTabs => "New tabs",
            Self::Terminal => "Terminal",
            Self::Window => "Window and title bar",
            Self::BackgroundSessions => {
                if cfg!(feature = "zmux") {
                    "Background sessions (zmux)"
                } else {
                    "Background sessions"
                }
            }
            Self::RemoteSessions => "Remote sessions",
            #[cfg(servers_enabled)]
            Self::NetworkServices => "Network services",
            Self::Profiles => "Profiles",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::NewTabs => "What a new tab runs, and where it starts",
            Self::Terminal => "How terminal text looks, and how much of it is kept",
            Self::Window => "Pane dimming, compact mode, and what the title bar shows",
            Self::BackgroundSessions => {
                if cfg!(feature = "session-persistence") {
                    "Screen retention for detached and shared sessions"
                } else {
                    "Screen retention for detached sessions"
                }
            }
            Self::RemoteSessions => "How panes of a session on another machine are carried",
            #[cfg(servers_enabled)]
            Self::NetworkServices => "Ports used by the optional local servers",
            Self::Profiles => "The shells and programs a tab can run",
        }
    }
}

impl ConfigSetting {
    /// The control the setting's row hosts: what the keyboard focuses, and
    /// what the row highlights for. Decided by the setting's kind, except for
    /// the settings with an editor of their own.
    pub(crate) fn control(self) -> SettingsControl {
        use SettingsControl as C;
        match self.spec().kind {
            SettingKind::Switch(_) => C::Toggle(SettingsToggle::Setting(self)),
            SettingKind::Choice(_) => C::Dropdown(SettingsDropdown::Setting(self)),
            SettingKind::Number(_) => C::Numeric(self),
            SettingKind::Text(_) => {
                C::Input(SettingsInput::Configuration(ConfigTextField::Setting(self)))
            }
            SettingKind::Custom(_) => match self {
                Self::DefaultProfile => C::Dropdown(SettingsDropdown::DefaultProfile),
                Self::LightTheme => C::Dropdown(SettingsDropdown::Theme),
                Self::DarkTheme => C::Dropdown(SettingsDropdown::DarkTheme),
                Self::DefaultTabIcon => C::DefaultTabIconPicker,
                Self::FontFamily => C::FontPicker,
                Self::InactivePaneOpacity => C::Opacity(OpacityTarget::Configuration),
                _ => unreachable!("{self:?} is not a custom setting"),
            },
        }
    }

    /// The element id of the row's control.
    pub(crate) fn element_id(self) -> SharedString {
        format!("settings-{self:?}").into()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DefaultProfile => "Default profile",
            Self::NewTabProfile => "New tab profile",
            Self::DefaultTabIcon => "Default tab icon",
            Self::WorkingDirectory => "Working directory",
            Self::WorkingDirectoryScope => "Inherit working directory",
            Self::LightTheme => "Light theme",
            Self::DarkTheme => "Dark theme",
            Self::FontSize => "Font size",
            Self::FontFamily => "Font family",
            Self::ScrollHistory => "Scrollback history",
            Self::InactivePaneOpacity => "Inactive pane opacity",
            Self::CompactMode => "Compact mode",
            Self::ShowPaneSize => "Show pane size",
            Self::ShowTitleBarLabels => "Show title bar labels",
            Self::ShowTitleBarButtons => "Show title bar buttons",
            #[cfg(target_os = "macos")]
            Self::ShowTitleBarMenus => "Show title bar menus",
            Self::PaneControlsPosition => "Pane controls position",
            Self::ShowPaneControls => "Show pane controls on new panes",
            Self::SessionRetention => "Detached session retention",
            Self::SessionRingBytes => "Retained screen size",
            #[cfg(feature = "session-persistence")]
            Self::DiskRecipients => "Disk recipients",
            #[cfg(feature = "session-persistence")]
            Self::IdentityFile => "Identity file",
            #[cfg(feature = "session-persistence")]
            Self::AutoProtect => "Protect sessions with your key",
            Self::RemoteProtocol => "Remote session protocol",
            Self::RemoteKeepAlive => "Remote keep-alive",
            Self::ForwardAgent => "Forward SSH agent",
            #[cfg(feature = "http-server")]
            Self::HttpServerPort => "HTTP server port",
            #[cfg(feature = "tftp-server")]
            Self::TftpServerPort => "TFTP server port",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::DefaultProfile => "The profile Zetta starts with",
            Self::NewTabProfile => {
                "Default opens the default profile; Inherit reuses the active tab's profile"
            }
            Self::DefaultTabIcon => "The icon new tabs get; choose None to show no icon",
            Self::WorkingDirectory => "Where new shells start; ~ is your home directory",
            Self::WorkingDirectoryScope => {
                "Which new shells start in the active pane's current directory instead"
            }
            Self::LightTheme => "The application theme while the system appearance is light",
            Self::DarkTheme => "The application theme while the system appearance is dark",
            Self::FontSize => "Points, from 6 through 100; leave empty to follow the theme",
            Self::FontFamily => "Search bundled and system-installed font families",
            Self::ScrollHistory => {
                "Lines, from 0 through Max; the steppers speed up as you hold them"
            }
            Self::InactivePaneOpacity => {
                "How much of a pane without focus stays visible; 100% does not dim it"
            }
            Self::CompactMode => "Move the tabs into the title bar and reduce its controls",
            Self::ShowPaneSize => {
                "Show the active pane's size in columns and rows in the title bar"
            }
            Self::ShowTitleBarLabels => {
                "Show text beside title bar icons, such as Menu, Profile and Keep running"
            }
            Self::ShowTitleBarButtons => {
                "Show title bar buttons such as Detach, and Keep running in --no-mux mode"
            }
            #[cfg(target_os = "macos")]
            Self::ShowTitleBarMenus => "Show the Menu and Profile menus in the title bar",
            Self::PaneControlsPosition => "Which side of a pane its overlay controls sit on",
            Self::ShowPaneControls => "Whether a new pane starts with its overlay controls shown",
            Self::SessionRetention => {
                if cfg!(feature = "session-persistence") {
                    "Keep no screen, a bounded screen in memory, or encrypted state on disk; \
                     disk falls back to memory while it is unavailable"
                } else {
                    "Keep no screen, or a bounded screen in memory"
                }
            }
            Self::SessionRingBytes => {
                "Bytes of screen a background session keeps, from 4 KiB through 64 MiB"
            }
            #[cfg(feature = "session-persistence")]
            Self::DiskRecipients => {
                "Comma-separated age recipients or github:USER entries that encrypted \
                 sessions are sealed to"
            }
            #[cfg(feature = "session-persistence")]
            Self::IdentityFile => {
                "The age identity that opens them; ~/.ssh/id_ed25519 is used when this is empty \
                 and that file exists"
            }
            #[cfg(feature = "session-persistence")]
            Self::AutoProtect => {
                "Detach, keep and share without a secret prompt: the session key is sealed to \
                 the recipients above and reopened with the identity file"
            }
            Self::RemoteProtocol => {
                "What carries a remote session's panes. Finding and attaching one is OpenSSH \
                 either way; Zosh gives each pane a Mosh link of its own, which survives \
                 roaming and suspend"
            }
            Self::RemoteKeepAlive => {
                "Milliseconds, from 20 through 3000, a Zosh link may go without sending before \
                 it holds itself open; leave empty for Mosh's own three-second heartbeat"
            }
            Self::ForwardAgent => {
                "Forward the local SSH agent through OpenSSH or Zosh; remote processes can use \
                 the forwarded socket"
            }
            #[cfg(feature = "http-server")]
            Self::HttpServerPort => "The TCP port the static HTTP server listens on",
            #[cfg(feature = "tftp-server")]
            Self::TftpServerPort => "The UDP port the TFTP server listens on",
        }
    }
}

/// One thing the Configuration page draws, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigurationItem {
    Heading(ConfigurationSection),
    Row(ConfigSetting),
    /// The macOS Focus status row: what access Zetta has, and the button that
    /// asks for it. Not a setting, so not in the settings table.
    #[cfg(target_os = "macos")]
    FocusStatus,
    /// The card of the configured profile at this index.
    Profile(usize),
    AddProfile,
}

/// The Configuration page, top to bottom.
///
/// A row is here only while the page draws it, so a control that cannot be
/// seen is never a tab stop: the automatic-protection switch, which needs a
/// recipient and an identity before it means anything, is the one that
/// depends on the form.
pub(crate) fn configuration_layout(editor: &SettingsEditor) -> Vec<ConfigurationItem> {
    use ConfigSetting as R;
    use ConfigurationItem::{Heading, Row};
    use ConfigurationSection as S;
    let mut items = vec![
        Heading(S::NewTabs),
        Row(R::DefaultProfile),
        Row(R::NewTabProfile),
        Row(R::DefaultTabIcon),
        Row(R::WorkingDirectory),
        Row(R::WorkingDirectoryScope),
        Heading(S::Terminal),
        Row(R::LightTheme),
        Row(R::DarkTheme),
        Row(R::FontSize),
        Row(R::FontFamily),
        Row(R::ScrollHistory),
        Heading(S::Window),
        Row(R::InactivePaneOpacity),
        Row(R::CompactMode),
        Row(R::ShowPaneSize),
        Row(R::ShowTitleBarLabels),
        Row(R::ShowTitleBarButtons),
        #[cfg(target_os = "macos")]
        Row(R::ShowTitleBarMenus),
        #[cfg(target_os = "macos")]
        ConfigurationItem::FocusStatus,
        Row(R::PaneControlsPosition),
        Row(R::ShowPaneControls),
        Heading(S::BackgroundSessions),
        Row(R::SessionRetention),
        Row(R::SessionRingBytes),
        #[cfg(feature = "session-persistence")]
        Row(R::DiskRecipients),
        #[cfg(feature = "session-persistence")]
        Row(R::IdentityFile),
    ];
    #[cfg(feature = "session-persistence")]
    if editor.configuration.session_auto_protect_is_offered() {
        items.push(Row(R::AutoProtect));
    }
    // Remote sessions need the multiplexer, and only a Zosh-capable client
    // has a protocol to choose or a Zosh link to keep alive. The keys still
    // parse and survive a save in other builds; they are just not offered.
    if cfg!(feature = "zmux") {
        items.push(Heading(S::RemoteSessions));
        if cfg!(feature = "zosh-client") {
            items.extend([Row(R::RemoteProtocol), Row(R::RemoteKeepAlive)]);
        }
        items.push(Row(R::ForwardAgent));
    }
    #[cfg(servers_enabled)]
    {
        items.push(Heading(S::NetworkServices));
        #[cfg(feature = "http-server")]
        items.push(Row(R::HttpServerPort));
        #[cfg(feature = "tftp-server")]
        items.push(Row(R::TftpServerPort));
    }
    items.push(Heading(S::Profiles));
    items.extend((0..editor.configuration.profiles.len()).map(ConfigurationItem::Profile));
    items.push(ConfigurationItem::AddProfile);
    items
}

/// The Configuration page's tab order, from [`configuration_layout`].
pub(crate) fn configuration_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
    configuration_layout(editor)
        .into_iter()
        .flat_map(|item| match item {
            ConfigurationItem::Heading(_) => Vec::new(),
            ConfigurationItem::Row(row) => vec![row.control()],
            #[cfg(target_os = "macos")]
            ConfigurationItem::FocusStatus => vec![SettingsControl::RequestFocusStatusAccess],
            ConfigurationItem::Profile(index) => {
                let profile = &editor.configuration.profiles[index];
                controls::profile_controls(index, profile.detected, profile.arguments.len())
            }
            ConfigurationItem::AddProfile => vec![SettingsControl::AddProfile],
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/settings_ui/configuration_page.rs"]
mod tests;
