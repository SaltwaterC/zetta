//! Per-profile Windows console selection. Default is the embedded console server,
//! unless the configuration file's top-level `windows_console_backend` names
//! another for every profile without a choice of its own.
//! This travels with a resolved command, so the process which spawns it chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsoleBackend {
    Conpty,
    #[default]
    ConsoleServer,
}
impl ConsoleBackend {
    pub fn is_default(&self) -> bool {
        *self == Self::ConsoleServer
    }
    /// The value as the configuration file spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conpty => "conpty",
            Self::ConsoleServer => "console-server",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Conpty, Self::ConsoleServer]
            .into_iter()
            .find(|backend| backend.as_str() == value)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Conpty => "ConPTY",
            Self::ConsoleServer => "Console server",
        }
    }
    /// What a profile without a choice of its own shows while this is the
    /// configured default.
    pub fn default_label(self) -> &'static str {
        match self {
            Self::Conpty => "Default (ConPTY)",
            Self::ConsoleServer => "Default (Console server)",
        }
    }
}
