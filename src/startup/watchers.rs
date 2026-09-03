//! The pollers a running process keeps.
//!
//! Both watch a file's metadata rather than its contents and only read when
//! the stamp changes, so an idle process does no parsing: the configuration
//! and keymap files, which may be edited outside the settings UI, and the
//! multiplexer's published session catalog, which is what the reconnect list
//! is built from.

use super::*;

/// How often the configuration file is checked for changes made outside the
/// settings UI. The check is metadata-only while the file is unchanged, so it
/// does not add work to rendering or input handling.
const CONFIGURATION_FILE_POLL: Duration = Duration::from_secs(1);

pub(super) fn config_file_stamp(path: &Path) -> ConfigFileStamp {
    let Ok(metadata) = fs::metadata(path) else {
        return ConfigFileStamp {
            modified: None,
            len: 0,
        };
    };
    ConfigFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    }
}

pub(super) fn reload_process_configuration(cx: &mut App) -> Result<()> {
    let (config_path, keymap_override) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config.keymap_override.clone(),
        )
    };
    let config_stamp = config_file_stamp(&config_path);
    let config = Config::load(Some(&config_path), keymap_override)?;
    let entities = process_zetta_entities(cx);
    let has_entities = !entities.is_empty();
    for entity in entities {
        entity
            .update(cx, |zetta, cx| {
                zetta.reload_configuration_from_process(config.clone(), cx)
            })
            .with_context(|| {
                format!("applying reloaded configuration {}", config_path.display())
            })?;
    }
    // A process can receive a request before its first window has been
    // attached. Keep launcher integrations correct in that small window too;
    // normal entities update them as part of their reload path.
    if !has_entities {
        #[cfg(windows)]
        windows_integration::update_profile_jump_list(
            config.profiles.clone(),
            config.hidden_profiles.clone(),
        );
        #[cfg(target_os = "linux")]
        if linux_desktop::update_profile_actions(&config.profiles, &config.hidden_profiles)
            .log_err()
            .unwrap_or(false)
        {
            schedule_linux_desktop_window_reassociation(cx);
        }
        #[cfg(target_os = "macos")]
        update_native_macos_dock_menu(cx, &config.profiles, &config.hidden_profiles);
    }
    let process = cx.global_mut::<ZettaProcessState>();
    process.config = config;
    process.config_file_stamp = config_stamp;
    process.configuration_error = None;
    Ok(())
}

pub(super) fn reload_process_configuration_if_changed(cx: &mut App) -> Result<bool> {
    let (config_path, last_stamp) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config_file_stamp,
        )
    };
    if config_file_stamp(&config_path) == last_stamp {
        return Ok(false);
    }
    reload_process_configuration(cx)?;
    Ok(true)
}

/// Keeps every open window, native launcher, and the process-wide launch
/// configuration in sync with edits made directly to config.json. Profile
/// lists are read during this idle watcher rather than during rendering.
pub(super) fn start_configuration_watcher(cx: &mut App) {
    let (config_path, mut last_seen) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config_file_stamp,
        )
    };
    #[cfg(target_os = "linux")]
    let mut desktop_entry_stamp = linux_desktop::desktop_entry_stamp();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(CONFIGURATION_FILE_POLL)
                .await;
            let changed = config_file_stamp(&config_path);
            if changed != last_seen {
                last_seen = changed;
                if let Err(error) = cx.update(reload_process_configuration) {
                    eprintln!(
                        "Could not reload {} after it changed: {error:#}",
                        config_path.display()
                    );
                }
                #[cfg(target_os = "linux")]
                {
                    // A configuration reload can update the desktop entry
                    // itself. Absorb that write so the desktop poll below
                    // does not schedule a second repair for the same change.
                    desktop_entry_stamp = linux_desktop::desktop_entry_stamp();
                }
            }

            #[cfg(target_os = "linux")]
            {
                let current_stamp = linux_desktop::desktop_entry_stamp();
                if current_stamp != desktop_entry_stamp {
                    desktop_entry_stamp = current_stamp;
                    // An installer may atomically replace the entry with
                    // byte-for-byte identical content. That still causes
                    // GNOME Shell to refresh its app cache, so repair any
                    // managed entry replacement rather than relying on a
                    // content diff.
                    if linux_desktop::is_managed_user_desktop_entry() {
                        cx.update(schedule_linux_desktop_window_reassociation);
                    }
                }
            }
        }
    })
    .detach();
}

/// How often the multiplexer's published catalog is checked for changes.
const MULTIPLEXER_CATALOG_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogFileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogStamp {
    catalog: Option<SessionCatalogFileStamp>,
    persistence_manifest: Option<SessionCatalogFileStamp>,
}

fn session_catalog_file_stamp(path: &Path) -> Option<SessionCatalogFileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(SessionCatalogFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

fn session_catalog_stamp(directory: &Path) -> SessionCatalogStamp {
    SessionCatalogStamp {
        catalog: session_catalog_file_stamp(directory),
        persistence_manifest: session_catalog_file_stamp(
            &directory.join("persistence").join("manifest.json"),
        ),
    }
}

/// Notices sessions the multiplexer is holding.
///
/// The reconnect list used to be refreshed only when *this* process published
/// its own catalog. Once the multiplexer owns the sessions that stopped
/// happening, so a window that had not detached anything itself never learned
/// that anything was there — no reconnect button, and the action finding
/// nothing to offer.
///
/// The catalog is a file the multiplexer replaces atomically, so this watches
/// the directory's modification time and the persistence manifest's
/// modification time, and only re-reads when either changes. The manifest is
/// nested below the catalog directory, so watching the directory alone misses
/// a disk record being consumed by `resume`. That keeps an idle process from
/// parsing the catalog and scanning the process table once a second for no
/// reason while still invalidating both live-session and disk-session entries.
pub(super) fn start_multiplexer_session_watcher(cx: &mut App) {
    let directory = crate::background_sessions::session_catalog_dir();
    let mut last_seen: Option<SessionCatalogStamp> = None;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(MULTIPLEXER_CATALOG_POLL)
                .await;
            let changed = session_catalog_stamp(&directory);
            // A first look always refreshes: the catalog may already describe
            // sessions from before this process started.
            if last_seen.is_some_and(|last_seen| changed == last_seen) {
                continue;
            }
            last_seen = Some(changed);
            cx.update(refresh_process_background_sessions);
        }
    })
    .detach();
}

pub(crate) fn refresh_process_background_sessions(cx: &mut App) {
    let entities = process_zetta_entities(cx);
    let mut entries = Vec::new();
    for zetta in &entities {
        let zetta = zetta.read(cx);
        let runner_id = zetta.background_sessions.runner_id();
        entries.extend(zetta.background_session_picker_entries.iter().map(
            |(session_id, title, details)| (runner_id, *session_id, title.clone(), details.clone()),
        ));
    }
    let no_mux = cx.has_global::<ZettaProcessState>() && cx.global::<ZettaProcessState>().no_mux;
    if !no_mux {
        entries.extend(multiplexer_session_entries());
    }
    if cx.has_global::<ZettaProcessState>() {
        cx.global_mut::<ZettaProcessState>()
            .background_session_entries = entries.into();
    }
    for zetta in entities {
        zetta.update(cx, |_, cx| cx.notify());
    }
}

pub(crate) fn prune_empty_dormant_runners(cx: &mut App) {
    if !cx.has_global::<ZettaProcessState>() {
        return;
    }
    let dormant = std::mem::take(&mut cx.global_mut::<ZettaProcessState>().dormant);
    let mut retained = Vec::with_capacity(dormant.len());
    let mut removed_runner_ids = Vec::new();
    for zetta in dormant {
        let (is_empty, runner_id) = {
            let state = zetta.read(cx);
            (
                state.background_sessions.is_empty(),
                state.background_sessions.runner_id(),
            )
        };
        if is_empty {
            removed_runner_ids.push(runner_id);
        } else {
            retained.push(zetta);
        }
    }
    let process = cx.global_mut::<ZettaProcessState>();
    process.dormant = retained;
    for runner_id in removed_runner_ids {
        process.runners.remove(&runner_id);
    }
    if should_quit_after_window_closed(process.windows.len(), process.dormant.len()) {
        quit_zetta_process(cx);
    }
}

/// The sessions the multiplexer is holding, as reconnect entries.
///
/// Read from the published catalog rather than by asking the multiplexer,
/// because this runs whenever the session list might have changed and must not
/// cost a round trip. Catalogs published by *this* process are skipped: those
/// describe sessions kept in memory here because the multiplexer was
/// unreachable, and they are already in the list.
fn multiplexer_session_entries() -> Vec<ProcessBackgroundSessionEntry> {
    let catalogs = match crate::background_sessions::read_session_catalogs(
        &crate::background_sessions::session_catalog_dir(),
    ) {
        Ok(catalogs) => catalogs,
        Err(error) => {
            log::debug!("could not read the session catalog: {error:#}");
            return Vec::new();
        }
    };
    // Only the multiplexer's own catalog counts: a Zetta process that kept a
    // session in memory because the multiplexer was unreachable publishes one
    // too, and those sessions are this process's to transfer, not the daemon's
    // to attach.
    let entries = crate::background_sessions::multiplexer_held_catalog_sessions(
        &catalogs,
        crate::background_sessions::process_is_zetta,
        std::process::id(),
    )
    .map(|(catalog, session)| {
        let runner_id = catalog.runner_id;
        let details = if session.authentication_required {
            format!("Session {} · protected", session.id)
        } else {
            let applications = session
                .panes
                .iter()
                .map(|pane| pane.application.as_str())
                .collect::<Vec<_>>();
            let panes = session.panes.len();
            let mut details = format!(
                "Session {} · {panes} pane{}",
                session.id,
                if panes == 1 { "" } else { "s" }
            );
            if !applications.is_empty() {
                details.push_str(" · ");
                details.push_str(&applications.join(", "));
            }
            details
        };
        (runner_id, session.id, session.title.clone(), details)
    })
    .collect::<Vec<_>>();
    #[cfg(feature = "session-persistence")]
    let mut entries = entries;
    #[cfg(feature = "session-persistence")]
    if let Ok(records) =
        zmux::persistence::read_opaque_records(&crate::background_sessions::session_catalog_dir())
    {
        let live_ids = entries.iter().map(|(_, session_id, _, _)| *session_id);
        let live_ids = live_ids.collect::<std::collections::HashSet<_>>();
        entries.extend(
            records
                .into_iter()
                .filter(|record| record.restorable && !live_ids.contains(&record.id))
                .map(|record| {
                    (
                        crate::background_sessions::RESTORABLE_RUNNER_ID,
                        record.id,
                        "Restorable session".to_owned(),
                        format!(
                            "Session {} · encrypted disk record{}",
                            record.id,
                            if record.protected {
                                " · protected"
                            } else {
                                ""
                            }
                        ),
                    )
                }),
        );
    }
    entries
}

#[cfg(test)]
#[path = "../tests/startup/watchers.rs"]
mod tests;
