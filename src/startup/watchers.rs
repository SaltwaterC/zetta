//! The pollers a running process keeps.
//!
//! Both probe metadata off-thread rather than contents and only read when
//! the stamp changes, so an idle process does no parsing: the configuration
//! and keymap files, which may be edited outside the settings UI, and the
//! multiplexer's published session catalog, which is what the reconnect list
//! is built from.

use super::*;

#[cfg(feature = "zmux")]
use std::sync::atomic::{AtomicBool, Ordering};

/// How often the configuration file is checked for changes made outside the
/// settings UI. The check is metadata-only while the file is unchanged, so it
/// does not add work to rendering or input handling.
const CONFIGURATION_FILE_POLL: Duration = Duration::from_secs(1);

pub(crate) fn config_file_stamp(path: &Path) -> ConfigFileStamp {
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

/// Reloads the process's configuration into every window, and calls
/// `completion` once that has been committed.
///
/// The reading happens on a worker; see `configuration_reload::coordinator`.
/// Every window keeps its current configuration until then.
pub(crate) fn reload_process_configuration(
    cx: &mut App,
    completion: impl FnOnce(Result<()>, &mut App) + 'static,
) {
    crate::configuration_reload::request_configuration_reload(
        crate::configuration_reload::ReloadScope::Process,
        Box::new(move |outcome, cx| completion(outcome.process_result(), cx)),
        cx,
    );
}

/// [`reload_process_configuration`] if the file changed since it was last
/// loaded; `completion` learns whether it had. The check itself is a `stat`,
/// so it is made off the GUI thread too.
pub(super) fn reload_process_configuration_if_changed(
    cx: &mut App,
    completion: impl FnOnce(Result<bool>, &mut App) + 'static,
) {
    let config_path = cx.global::<ZettaProcessState>().config.config_path.clone();
    cx.spawn(async move |cx| {
        let stamp = cx
            .background_spawn(async move { config_file_stamp(&config_path) })
            .await;
        cx.update(|cx| {
            // Compared now rather than before the `stat`: a reload that
            // committed meanwhile has already recorded this stamp.
            if stamp == cx.global::<ZettaProcessState>().config_file_stamp {
                completion(Ok(false), cx);
                return;
            }
            reload_process_configuration(cx, move |result, cx| {
                completion(result.map(|()| true), cx);
            });
        });
    })
    .detach();
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
            // `cx.spawn` resumes on the foreground thread, so taking these
            // stamps inline put two or three `stat` calls per second on the
            // thread that draws. Only a change needs the foreground.
            let changed = {
                let config_path = config_path.clone();
                cx.background_spawn(async move { config_file_stamp(&config_path) })
                    .await
            };
            if changed != last_seen {
                last_seen = changed;
                let (sender, reloaded) = futures::channel::oneshot::channel();
                cx.update(|cx| {
                    reload_process_configuration(cx, move |result, _| {
                        let _ = sender.send(result);
                    });
                });
                if let Ok(Err(error)) = reloaded.await {
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
                    desktop_entry_stamp = cx
                        .background_spawn(async { linux_desktop::desktop_entry_stamp() })
                        .await;
                }
            }

            #[cfg(target_os = "linux")]
            {
                let current_stamp = cx
                    .background_spawn(async { linux_desktop::desktop_entry_stamp() })
                    .await;
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
#[cfg(feature = "zmux")]
const MULTIPLEXER_CATALOG_POLL: Duration = Duration::from_secs(1);

#[cfg(feature = "zmux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogFileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[cfg(feature = "zmux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogStamp {
    catalog: Option<SessionCatalogFileStamp>,
    persistence_manifest: Option<SessionCatalogFileStamp>,
}

#[cfg(feature = "zmux")]
fn session_catalog_file_stamp(path: &Path) -> Option<SessionCatalogFileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(SessionCatalogFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

#[cfg(feature = "zmux")]
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
#[cfg(feature = "zmux")]
pub(super) fn start_multiplexer_session_watcher(cx: &mut App) {
    let directory = crate::background_sessions::session_catalog_dir();
    let mut last_seen: Option<SessionCatalogStamp> = None;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(MULTIPLEXER_CATALOG_POLL)
                .await;
            let changed = {
                let directory = directory.clone();
                cx.background_spawn(async move { session_catalog_stamp(&directory) })
                    .await
            };
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

/// Whether a catalog read is already running, and whether another was asked for
/// while it was.
///
/// Session transitions and the stamp watcher can request reads together;
/// without this each would start its own read of the same directory. Local
/// title changes only recombine the cached entries, without requesting I/O.
#[cfg(feature = "zmux")]
static CATALOG_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "zmux")]
static CATALOG_REFRESH_PENDING: AtomicBool = AtomicBool::new(false);

/// Rebuilds the process-wide reconnect list.
///
/// The multiplexer's half of that list costs a `read_dir`, a JSON parse per
/// published catalog, a `stat` per catalog to tell a Zetta process from the
/// daemon, and — with session persistence — a read of the encrypted disk
/// records. That ran on the thread that draws, on every title change of every
/// background pane. It now runs on the background executor and only the
/// combining step returns to the foreground.
#[cfg(feature = "zmux")]
pub(crate) fn refresh_process_background_sessions(cx: &mut App) {
    if CATALOG_REFRESH_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        CATALOG_REFRESH_PENDING.store(true, Ordering::Release);
        return;
    }
    let no_mux = cx.has_global::<ZettaProcessState>() && cx.global::<ZettaProcessState>().no_mux;
    cx.spawn(async move |cx| {
        let multiplexer_entries = if no_mux {
            Vec::new()
        } else {
            cx.background_spawn(async { multiplexer_session_entries() })
                .await
        };
        cx.update(|cx| {
            cx.set_global(MultiplexerPickerEntries(multiplexer_entries));
            refresh_local_background_sessions(cx);
        });
        CATALOG_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
        // A refresh requested while the read was in flight described state
        // this result cannot include, so it gets its own read rather than being
        // dropped.
        if CATALOG_REFRESH_PENDING.swap(false, Ordering::AcqRel) {
            cx.update(refresh_process_background_sessions);
        }
    })
    .detach();
}

#[derive(Default)]
struct MultiplexerPickerEntries(Vec<ProcessBackgroundSessionEntry>);

impl Global for MultiplexerPickerEntries {}

/// Combines current local state with the last daemon read. Local title and
/// detach/reconnect changes do not wait for a catalog write or read to finish.
pub(crate) fn refresh_local_background_sessions(cx: &mut App) {
    let entities = process_zetta_entities(cx);
    let mut entries = Vec::new();
    for zetta in &entities {
        let zetta = zetta.read(cx);
        let runner_id = zetta.background_sessions.runner_id();
        entries.extend(zetta.background_session_picker_entries.iter().map(
            |(session_id, title, details)| (runner_id, *session_id, title.clone(), details.clone()),
        ));
    }
    if let Some(multiplexer) = cx.try_global::<MultiplexerPickerEntries>() {
        entries.extend(multiplexer.0.iter().cloned());
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
    if should_quit_after_window_closed(
        process.windows.len(),
        process.dormant.len(),
        process.closing.len(),
    ) {
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
#[cfg(feature = "zmux")]
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
    #[cfg(feature = "session-persistence")]
    let live_mux_ids = crate::background_sessions::multiplexer_catalog_session_ids(
        &catalogs,
        crate::background_sessions::process_is_zetta,
    )
    .collect::<std::collections::HashSet<_>>();
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
        entries.extend(
            records
                .into_iter()
                .filter(|record| record.restorable && !live_mux_ids.contains(&record.id))
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

#[cfg(all(test, feature = "zmux"))]
#[path = "../tests/startup/watchers.rs"]
mod tests;
