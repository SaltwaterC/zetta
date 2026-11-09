# Compatibility and format versions

## Application version

Change only `[package].version` in the root `Cargo.toml` to bump Zetta's
version, then rebuild. Cargo supplies it through `CARGO_PKG_VERSION` to the
CLI, application state, terminal `TERM_PROGRAM_VERSION`, HTTP user agent, and
performance reports. The macOS bundle also reads its version from that manifest.
Cargo refreshes the root package's generated `Cargo.lock` entry on the next
build; do not edit the lockfile's version by hand.

Crates under `crates/` keep independently maintained package versions, including
forks that happen to have the same version as Zetta. Compatibility and format
markers below are also versioned independently.

## Version output

`zetta -v` (or `zetta --version`) prints the package version followed by named
compatibility markers. A default Linux build prints the following, with
`<package-version>` taken from the root manifest:

```text
Zetta <package-version>
CONTROL_VERSION=1
CATALOG_VERSION=1
ZMUX_PROTOCOL_VERSION=1
ZMUX_ENDPOINT_VERSION=1
ZMUX_SHARED_SESSION_STATE_VERSION=1
ZMUX_HANDOVER_VERSION=1
ZMUX_MANIFEST_VERSION=1
ZMUX_PERSISTENCE_BINDING_VERSION=1
ZMUX_SEALED_KEY_VERSION=1
PROJECT_REGISTRY_VERSION=1
PERFORMANCE_REPORT_VERSION=1
ZSH_LIFECYCLE_TRACKING_VERSION=1
ZOSH_AGENT_PROTOCOL_VERSION=1
ZOSH_CLIPBOARD_PROTOCOL_VERSION=1
```

All Zetta-owned compatibility and format versions start at **1** for the initial
release. This resets the development counters while retaining the current
formats and behavior. Development builds and their saved data are not a
supported upgrade path across this reset; start new sessions with matching
binaries. These counters are independent of the executable package version.

The output reflects the platform and compiled features. Builds without `zmux`
still report the local process-control and catalog versions. Persistence markers
require `session-persistence`; Zosh extension markers require `zosh-client`.
Windows builds with `zmux` also report the host markers below. `zmux -v` and
`zetta mux -v` print the same multiplexer markers as `zetta -v`, after their
package/protocol summary.

## Version inventory

Every marker in this table currently has value **1**. Values come from the
constants used by the corresponding readers and writers, so version output can
be included directly in a bug report.

| Output marker | Owner | Contract |
| --- | --- | --- |
| `CONTROL_VERSION` | `crates/zmux/src/protocol.rs`; fallback in `src/process_control.rs` | Local Zetta process-control endpoint and command meanings. Peers require an exact match. |
| `CATALOG_VERSION` | `crates/zmux/src/protocol.rs`; fallback in `src/local_sessions.rs` | Published background-session catalog JSON. Unknown catalog versions are ignored. |
| `ZMUX_PROTOCOL_VERSION` | `crates/zmux/src/messages.rs` | Client/daemon messages. Normal requests require an exact match; debug session directories are namespaced by this value. |
| `ZMUX_ENDPOINT_VERSION` | `crates/zmux/src/transport.rs` | Daemon and Windows host endpoint descriptors. Unknown versions are rejected. |
| `ZMUX_SHARED_SESSION_STATE_VERSION` | `crates/zmux/src/messages.rs` | Durable shared-session state, including revisions, geometry, focus, and opaque tab payload. Unknown versions are rejected. |
| `ZMUX_HANDOVER_VERSION` | `crates/zmux/src/upgrade.rs`, `upgrade_windows.rs` | Platform-specific private state passed to a replacement daemon. The replacement is preflighted before the old image stops. |
| `ZMUX_MANIFEST_VERSION` | `crates/zmux/src/persistence.rs` | Encrypted session-store manifest schema. |
| `ZMUX_PERSISTENCE_BINDING_VERSION` | `crates/zmux/src/persistence/binding.rs` | Header binding an encrypted payload to its record, generation, pane, and payload kind. |
| `ZMUX_SEALED_KEY_VERSION` | `crates/zmux/src/auto_protect.rs` | Host-bound sealed session-key plaintext inside an age envelope. Unknown versions are rejected; unversioned bare keys remain restricted to local use. |
| `ZMUX_HOST_PROTOCOL_VERSION` | `crates/zmux/src/pty_host.rs` (Windows) | Additive protocol between a daemon and its long-lived console host. |
| `ZMUX_MINIMUM_HOST_PROTOCOL_VERSION` | `crates/zmux/src/pty_host.rs` (Windows) | Oldest host protocol the daemon can drive. This is a compatibility floor. |
| `ZMUX_CONSOLE_SERVER_HOST_PROTOCOL_VERSION` | `crates/zmux/src/pty_host.rs` (Windows) | First host protocol supporting console-server consoles. |
| `ZMUX_HOST_INSTALL_REVISION` | `crates/zmux/src/pty_host.rs`, `resources/windows/zmux-pty.version` (Windows) | Installer marker for host executable bytes; independent of the wire protocol. Bump when a host behavior fix needs deployment. |
| `PROJECT_REGISTRY_VERSION` | `src/project.rs` | Local `projects.json`, including registered roots and saved project approvals. |
| `PERFORMANCE_REPORT_VERSION` | `src/performance.rs` | `schema_version` in `zetta benchmark --profile-report` JSON. |
| `ZSH_LIFECYCLE_TRACKING_VERSION` | `crates/zetta_profiles/src/lib.rs`, `src/shell_integration/zsh.zsh` | Revision used to refresh Zsh command-lifecycle hooks. |
| `ZOSH_AGENT_PROTOCOL_VERSION` | `crates/zosh/src/agent.rs` | Zosh SSH-agent forwarding extension. |
| `ZOSH_CLIPBOARD_PROTOCOL_VERSION` | `crates/mosh_rs/src/session.rs` | Zosh clipboard query relay extension. |

## Other versions

External versions retain their specified values: Mosh protocol **2**, age file
format **1**, Argon2 version **19**, operating-system and graphics API versions,
and the external theme index's `max_schema_version` query. Dependency package
versions and upstream GPUI profiling formats are also independent of Zetta's
release baseline and are not printed as Zetta compatibility markers.

The disposable macOS notification-audio cache uses a `-v1.wav` filename suffix
in `crates/zntfy/src/sounds.rs`. It only invalidates regenerated audio and is not
a peer protocol or persisted user-data contract.

When changing a contract, update its owning constant and regression tests.
The shared multiplexer inventory lives in `crates/zmux/src/versions.rs`; the
application adds its local markers in `src/startup/cli_help.rs`. Keep the Windows
host installer marker synchronized with `HOST_INSTALL_REVISION`.
