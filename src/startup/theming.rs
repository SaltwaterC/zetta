//! Loading themes and applying the configuration a launch resolved.
//!
//! Zetta's overrides are baked into every theme as it is registered rather than
//! applied at each read, so a theme is already correct by the time a window
//! renders from it.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ThemeFileStamp {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

pub(crate) fn changed_theme_files(
    themes_dir: &Path,
    cache: &mut HashMap<PathBuf, ThemeFileStamp>,
) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut present = std::collections::HashSet::new();
    for entry in fs::read_dir(themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let stamp = ThemeFileStamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        present.insert(path.clone());
        if cache.get(&path) != Some(&stamp) {
            cache.insert(path.clone(), stamp);
            changed.push(path);
        }
    }
    cache.retain(|path, _| present.contains(path));
    Ok(changed)
}

pub(crate) fn load_user_themes(cx: &mut App) -> Result<()> {
    static THEME_FILE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ThemeFileStamp>>> = OnceLock::new();
    let themes_dir = config::themes_dir();
    fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating theme directory {}", themes_dir.display()))?;
    let registry = ThemeRegistry::global(cx);
    let paths = changed_theme_files(
        &themes_dir,
        &mut THEME_FILE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )?;
    for path in paths {
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        theme_settings::load_user_theme(&registry, &bytes)
            .with_context(|| format!("loading theme {}", path.display()))?;
    }
    Ok(())
}

/// Zetta's scrollbar colors, which every theme it ships or installs gets.
///
/// Idempotent: each field derives from a `text*` color rather than from itself,
/// so re-running it over an already-overridden theme is a no-op. That is what
/// lets [`bake_zetta_theme_overrides`] re-sweep the whole registry after a
/// reload without having to track which themes it already visited.
pub(crate) fn apply_zetta_theme_overrides(theme: &mut Theme) {
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
}

/// Whether [`apply_zetta_theme_overrides`] would leave this theme unchanged.
///
/// Derived from the same expressions the override writes, so the two cannot
/// disagree about what "already baked" means.
fn zetta_theme_overrides_are_baked(theme: &Theme) -> bool {
    let colors = &theme.styles.colors;
    colors.scrollbar_thumb_background == colors.text_muted.opacity(0.7)
        && colors.scrollbar_thumb_hover_background == colors.text.opacity(0.85)
        && colors.scrollbar_thumb_active_background == colors.text_accent.opacity(0.95)
}

/// Rewrites every registered theme with [`apply_zetta_theme_overrides`] applied.
///
/// The overrides used to be applied at each lookup instead, which cloned a whole
/// `Theme` every time. `window_theme`/`theme_for_tab` resolve a theme per tab per
/// frame, so that put one full theme clone per tab into every frame. Baking the
/// overrides into the registry reduces a lookup to a lock read and an `Arc` clone.
///
/// Call this after anything that can add themes to the registry; `apply_config_settings`
/// already does, and every reload path goes through it.
pub(crate) fn bake_zetta_theme_overrides(registry: &ThemeRegistry) {
    // Only themes that still need it are cloned. This runs at startup before
    // the first frame and again on every configuration reload, over every
    // bundled and installed theme, and a `Theme` clone carries its colours and
    // its whole syntax map. After the first sweep essentially every theme is
    // already baked, so the common case becomes three comparisons each.
    let overridden = registry
        .list_names()
        .into_iter()
        .filter_map(|name| registry.get(&name).ok())
        .filter(|theme| !zetta_theme_overrides_are_baked(theme))
        .map(|theme| {
            let mut theme = theme.as_ref().clone();
            apply_zetta_theme_overrides(&mut theme);
            theme
        })
        .collect::<Vec<_>>();
    if overridden.is_empty() {
        return;
    }
    registry.insert_themes(overridden);
}

pub(crate) fn resolve_profile_theme(profile: &Profile, cx: &App) -> Result<Option<Arc<Theme>>> {
    let configured_theme = if SystemAppearance::global(cx).is_light() {
        profile.theme.as_deref()
    } else {
        profile.dark_theme.as_deref()
    };
    configured_theme
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .with_context(|| format!("using theme {name:?} for profile {:?}", profile.name))
        })
        .transpose()
}

pub(crate) fn apply_config_settings(config: &Config, cx: &mut App) -> Result<()> {
    let registry = ThemeRegistry::global(cx);
    bake_zetta_theme_overrides(&registry);
    let theme_name = selected_theme_name_for_appearance(config, cx);
    let theme = registry
        .get(theme_name)
        .with_context(|| format!("using Zed theme {theme_name:?}"))?;
    GlobalTheme::update_theme(cx, theme);

    let mut terminal_settings = TerminalSettings::get_global(cx).clone();
    terminal_settings.font_family = Some(theme_settings::FontFamilyName(
        config.terminal_font_family.clone().into(),
    ));
    terminal_settings.font_size = config.terminal_font_size.map(px);
    terminal_settings.copy_on_select = true;
    terminal_settings.max_scroll_history_lines = Some(config.max_scroll_history_lines);
    TerminalSettings::override_global(terminal_settings, cx);
    Ok(())
}

pub(crate) fn selected_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_THEME)
}

pub(crate) fn selected_dark_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_DARK_THEME)
}

pub(crate) fn selected_theme_name_for_appearance<'a>(config: &'a Config, cx: &App) -> &'a str {
    if SystemAppearance::global(cx).is_light() {
        selected_theme_name(config.theme.as_deref())
    } else {
        selected_dark_theme_name(config.dark_theme.as_deref())
    }
}

pub(crate) fn normalize_keymap_key_names(content: &str) -> String {
    let content = content
        .replace("page-up", "pageup")
        .replace("page-down", "pagedown");
    let Ok(mut root) = serde_json::from_str::<Value>(&content) else {
        return content;
    };
    let Some(sections) = root.as_array_mut() else {
        return content;
    };

    let mut changed = false;
    for section in sections {
        let Some(bindings) = section.get_mut("bindings").and_then(Value::as_object_mut) else {
            continue;
        };
        let entries = std::mem::take(bindings);
        for (keystroke, action) in entries {
            let normalized = keymap_keystroke_storage(&keystroke);
            changed |= normalized != keystroke;
            bindings.insert(normalized, action);
        }
    }

    if changed {
        serde_json::to_string(&root).unwrap_or(content)
    } else {
        content
    }
}

pub(crate) fn validate_keymap_contents(content: &str, cx: &mut App) -> Result<()> {
    let content = normalize_keymap_key_names(content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { .. } => Ok(()),
        KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
            anyhow::bail!("some key bindings are invalid: {error_message}")
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            Err(error).context("parsing keymap JSON")
        }
    }
}

#[cfg(test)]
#[path = "../tests/startup/theming.rs"]
mod tests;
