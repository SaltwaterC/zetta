//! Zetta's keymap file model.
//!
//! This replaces Zed's `settings::KeymapFile`. That type is 2,800 lines built
//! around Zed's keymap *editor* — JSON schema generation, migration, rich
//! Markdown diagnostics — and it reached the `fs` crate, which is what dragged
//! Zed's git and SSH-askpass layers into a terminal emulator. Zetta uses four
//! entry points from it, so it owns them here instead.
//!
//! The file format is unchanged, so existing keymaps keep working: a JSON array
//! of sections, each with an optional `context` predicate and a `bindings` map
//! from keystroke sequence to action. An action is either a name, or a
//! `[name, input]` pair, or `null` to bind nothing. `//` comments are accepted,
//! which is why this parses leniently.

use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{
    Action, App, InvalidKeystrokeError, KeyBinding, KeyBindingContextPredicate, NoAction,
    SharedString,
};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

/// The bundled keymap that binds the shared menu and editor actions Zetta's
/// pickers and context menus dispatch. Platform-specific because the modifier
/// conventions differ.
#[cfg(target_os = "macos")]
pub(crate) const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-macos.json";
#[cfg(target_os = "windows")]
pub(crate) const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-windows.json";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-linux.json";

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub(crate) struct KeymapFile(Vec<KeymapSection>);

#[derive(Debug, Default, Deserialize)]
pub(crate) struct KeymapSection {
    /// When these bindings are active, as a context predicate. Empty means
    /// always.
    #[serde(default)]
    pub(crate) context: String,
    /// Position-equivalent mappings for non-QWERTY keyboards; macOS only.
    #[serde(default)]
    use_key_equivalents: bool,
    #[serde(default)]
    bindings: Option<IndexMap<String, KeymapAction>>,
}

/// One binding's action: a name, a `[name, input]` pair, or `null`.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub(crate) struct KeymapAction(Value);

pub(crate) enum KeymapFileLoadResult {
    Success {
        key_bindings: Vec<KeyBinding>,
    },
    /// Some bindings loaded and some did not. This is the normal result for the
    /// bundled keymap, which names actions Zetta does not register.
    SomeFailedToLoad {
        key_bindings: Vec<KeyBinding>,
        error_message: String,
    },
    JsonParseFailure {
        error: anyhow::Error,
    },
}

impl KeymapSection {
    pub(crate) fn bindings(&self) -> impl Iterator<Item = (&String, &KeymapAction)> {
        self.bindings.iter().flatten()
    }
}

impl KeymapFile {
    pub(crate) fn sections(&self) -> impl Iterator<Item = &KeymapSection> {
        self.0.iter()
    }

    pub(crate) fn parse(content: &str) -> Result<Self> {
        if content.trim().is_empty() {
            return Ok(Self(Vec::new()));
        }
        serde_json_lenient::from_str_lenient(content).context("parsing keymap")
    }

    /// The action a binding names, and its input if it has one.
    ///
    /// `Ok(None)` means the binding is explicitly `null` — bound to nothing.
    pub(crate) fn parse_action(
        action: &KeymapAction,
    ) -> std::result::Result<Option<(&String, Option<&Value>)>, String> {
        match &action.0 {
            Value::String(name) => Ok(Some((name, None))),
            Value::Null => Ok(None),
            Value::Array(items) => {
                let [name, input] = items.as_slice() else {
                    return Err(format!(
                        "expected a two-element array of [name, input], found {}",
                        action.0
                    ));
                };
                let Value::String(name) = name else {
                    return Err(format!(
                        "expected a two-element array of [name, input] whose first element is a \
                         string, found {}",
                        action.0
                    ));
                };
                Ok(Some((name, Some(input))))
            }
            other => Err(format!(
                "expected an action name or a two-element array of [name, input], found {other}"
            )),
        }
    }

    pub(crate) fn load(content: &str, cx: &App) -> KeymapFileLoadResult {
        match Self::parse(content) {
            Ok(keymap) => keymap.load_keymap(cx),
            Err(error) => KeymapFileLoadResult::JsonParseFailure { error },
        }
    }

    /// Loads a bundled keymap, keeping whatever bindings resolved.
    ///
    /// Partial failure is the expected case rather than an error: the bundled
    /// keymap names Zed actions that Zetta does not register, and those bindings
    /// are simply skipped.
    pub(crate) fn load_asset_allow_partial_failure(
        asset_path: &str,
        cx: &App,
    ) -> Result<Vec<KeyBinding>> {
        let asset = cx
            .asset_source()
            .load(asset_path)?
            .with_context(|| format!("keymap asset {asset_path} is missing"))?;
        let content = std::str::from_utf8(asset.as_ref())
            .with_context(|| format!("keymap asset {asset_path} is not UTF-8"))?;
        match Self::load(content, cx) {
            KeymapFileLoadResult::Success { key_bindings }
            | KeymapFileLoadResult::SomeFailedToLoad { key_bindings, .. } => Ok(key_bindings),
            KeymapFileLoadResult::JsonParseFailure { error } => {
                Err(error).with_context(|| format!("parsing keymap asset {asset_path}"))
            }
        }
    }

    /// Builds the bindings, accumulating per-binding errors rather than failing
    /// the file, so one bad entry cannot cost the user every other binding.
    fn load_keymap(&self, cx: &App) -> KeymapFileLoadResult {
        let mut key_bindings = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for section in &self.0 {
            let context_predicate: Option<Rc<KeyBindingContextPredicate>> =
                if section.context.is_empty() {
                    None
                } else {
                    match KeyBindingContextPredicate::parse(&section.context) {
                        Ok(predicate) => Some(Rc::new(predicate)),
                        Err(error) => {
                            errors.push(format!(
                                "in context {:?}: could not parse the context: {error}",
                                section.context
                            ));
                            continue;
                        }
                    }
                };

            for (keystrokes, action) in section.bindings() {
                match Self::load_binding(
                    keystrokes,
                    action,
                    context_predicate.clone(),
                    section.use_key_equivalents,
                    cx,
                ) {
                    Ok(key_binding) => key_bindings.push(key_binding),
                    Err(error) => errors.push(format!(
                        "in context {:?}, binding {keystrokes:?}: {error}",
                        section.context
                    )),
                }
            }
        }

        if errors.is_empty() {
            KeymapFileLoadResult::Success { key_bindings }
        } else {
            KeymapFileLoadResult::SomeFailedToLoad {
                key_bindings,
                error_message: errors.join("; "),
            }
        }
    }

    fn load_binding(
        keystrokes: &str,
        action: &KeymapAction,
        context: Option<Rc<KeyBindingContextPredicate>>,
        use_key_equivalents: bool,
        cx: &App,
    ) -> std::result::Result<KeyBinding, String> {
        let (action, action_input) = Self::build_action(action, cx)?;
        KeyBinding::load(
            keystrokes,
            action,
            context,
            use_key_equivalents,
            action_input.map(SharedString::from),
            cx.keyboard_mapper().as_ref(),
        )
        .map_err(|InvalidKeystrokeError { keystroke }| format!("invalid keystroke {keystroke:?}"))
    }

    fn build_action(
        action: &KeymapAction,
        cx: &App,
    ) -> std::result::Result<(Box<dyn Action>, Option<String>), String> {
        let (built, input) = match Self::parse_action(action)? {
            Some((name, Some(input))) => (
                cx.build_action(name, Some(input.clone())),
                Some(input.to_string()),
            ),
            Some((name, None)) => (cx.build_action(name, None), None),
            // An explicit `null` unbinds the keystroke.
            None => (Ok(NoAction.boxed_clone()), None),
        };
        match built {
            Ok(action) => Ok((action, input)),
            Err(error) => Err(format!("{error}")),
        }
    }
}

#[cfg(test)]
#[path = "tests/keymap_file.rs"]
mod tests;
