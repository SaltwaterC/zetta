//! The desktop's Do Not Disturb on Linux: GNOME's dconf key, which announces
//! its changes, or the KDE and XFCE D-Bus properties, which have to be polled.

use std::process::Command;

use gpui::App;

use super::{SystemSilentState, dconf_notify_affects, parse_gnome_show_banners};

/// Which desktop mechanism answered, so the observer knows whether a change
/// notification exists for it or whether it has to keep polling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxSilentSource {
    /// A D-Bus property that this code has no subscription for.
    DbusProperty,
    /// The `org.gnome.desktop.notifications show-banners` GSettings key, whose
    /// writes are announced by dconf.
    GnomeShowBanners,
}

pub(super) fn detect_linux_system_silent_state_with_source()
-> (SystemSilentState, LinuxSilentSource) {
    futures::executor::block_on(async {
        if let Some(inhibited) = query_kde_inhibited().await {
            return (
                if inhibited {
                    SystemSilentState::Active
                } else {
                    SystemSilentState::Inactive
                },
                LinuxSilentSource::DbusProperty,
            );
        }
        if let Some(inhibited) = query_xfce_inhibited().await {
            return (
                if inhibited {
                    SystemSilentState::Active
                } else {
                    SystemSilentState::Inactive
                },
                LinuxSilentSource::DbusProperty,
            );
        }
        (
            gnome_show_banners_state(),
            LinuxSilentSource::GnomeShowBanners,
        )
    })
}

/// The dconf key backing GNOME's Do Not Disturb switch.
const GNOME_SHOW_BANNERS_PATH: &str = "/org/gnome/desktop/notifications/show-banners";

/// Subscribes to dconf's change signal so the observer can park instead of
/// re-reading the setting on a timer.
///
/// The returned channel closes if the subscription cannot be established or is
/// later lost, which the caller treats as "resume polling".
pub(super) fn spawn_gnome_settings_watcher(cx: &App) -> async_channel::Receiver<()> {
    use futures::StreamExt as _;

    // A single slot is enough: the observer re-reads the current value on wake,
    // so coalesced notifications lose nothing.
    let (changes_tx, changes_rx) = async_channel::bounded(1);
    cx.background_executor()
        .spawn(async move {
            let Ok(connection) = zbus::Connection::session().await else {
                return;
            };
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("ca.desrt.dconf.Writer")
                .and_then(|builder| builder.member("Notify"))
                .and_then(|builder| builder.path("/ca/desrt/dconf/Writer/user"))
                .map(|builder| builder.build())
            else {
                return;
            };
            let Ok(mut messages) =
                zbus::MessageStream::for_match_rule(rule, &connection, Some(1)).await
            else {
                return;
            };

            while let Some(Ok(message)) = messages.next().await {
                let Ok((prefix, changed_keys, _tag)) = message
                    .body()
                    .deserialize::<(String, Vec<String>, String)>()
                else {
                    continue;
                };
                if !dconf_notify_affects(&prefix, &changed_keys, GNOME_SHOW_BANNERS_PATH) {
                    continue;
                }
                // A full slot already means "re-read pending"; dropping the
                // duplicate is correct. A closed channel means the observer is
                // gone, so the watcher can stop.
                match changes_tx.try_send(()) {
                    Ok(()) | Err(async_channel::TrySendError::Full(())) => {}
                    Err(async_channel::TrySendError::Closed(())) => break,
                }
            }
        })
        .detach();

    changes_rx
}

async fn query_kde_inhibited() -> Option<bool> {
    query_dbus_bool(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.DBus.Properties",
        "Get",
        &("org.freedesktop.Notifications", "Inhibited"),
    )
    .await
}

async fn query_xfce_inhibited() -> Option<bool> {
    query_dbus_bool(
        "org.xfce.Xfconf",
        "/org/xfce/Xfconf",
        "org.xfce.Xfconf",
        "GetProperty",
        &("xfce4-notifyd", "/do-not-disturb"),
    )
    .await
}

async fn query_dbus_bool<B>(
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Option<bool>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let connection = zbus::Connection::session().await.ok()?;
    let reply = connection
        .call_method(Some(destination), path, Some(interface), method, body)
        .await
        .ok()?;
    let (value,): (zbus::zvariant::OwnedValue,) = reply.body().deserialize().ok()?;
    bool::try_from(value).ok()
}

fn gnome_show_banners_state() -> SystemSilentState {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.notifications", "show-banners"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_gnome_show_banners(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or(SystemSilentState::Unknown)
}
