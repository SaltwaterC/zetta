# Compatibility and format versions

`zetta -v` begins with the normal package version and then prints the
compatibility markers that are most useful when diagnosing a running Zetta
installation:

```text
Zetta 0.1.0
CONTROL_VERSION=2
CATALOG_VERSION=1
ZMUX_PROTOCOL_VERSION=1
```

The package version identifies the executable. The three following values
identify contracts that can make otherwise similarly built processes refuse
one another or ignore persisted data. They are deliberately printed as named
fields so a bug report can include the complete block without relying on the
current source tree.

## Version inventory

| Marker | Current value | Owned by | What it versions | Compatibility effect |
| --- | ---: | --- | --- | --- |
| `CARGO_PKG_VERSION` | package version | Zetta and `zmux` | User-facing executable release | Identifies the build; it is not a wire-protocol negotiation value. |
| `CONTROL_VERSION` | `1` | `crates/zmux/src/protocol.rs` | The Zetta-to-Zetta process-control endpoint and request meanings, including disk-session resume, managed-worktree project opening, and the `zetta pane wait` exchange | Endpoints with another version are skipped. |
| `zmux::protocol::CATALOG_VERSION` | `1` | `crates/zmux/src/protocol.rs` | The public background-session catalog JSON | A catalog with another version is ignored until its owner publishes the current schema. |
| `zmux::messages::PROTOCOL_VERSION` | `1` | `crates/zmux/src/messages.rs` | The client/daemon message protocol, including disk-session resume and its length-prefixed transport framing | Normal requests require an exact match. `zmux --upgrade` is the compatibility path for replacing an older daemon. Debug session directories are namespaced by this value. |
| `zmux::transport::ENDPOINT_VERSION` | `1` | `crates/zmux/src/transport.rs` | The `zmux.json` endpoint descriptor (`socket_path`, token, process ID, and protocol advertisement) | An endpoint with an unknown shape is rejected, causing the client to recover by starting or finding a usable daemon. |
| `zmux::upgrade::HANDOVER_VERSION` | `5` on Unix, `1` on Windows | `crates/zmux/src/upgrade.rs`, `crates/zmux/src/upgrade_windows.rs` | The private state handed from one daemon image to the next during `--upgrade` | Unix carries descriptors through `execv`; Windows carries session metadata while `zmux-pty.exe` retains the consoles. The replacement is preflighted and refuses an unknown handover shape before the old daemon stops. |
| `zmux::pty_host::HOST_PROTOCOL_VERSION` | `1` on Windows | `crates/zmux/src/pty_host.rs` | The additive protocol between the Windows pseudoconsole host and a daemon | The host outlives a daemon replacement, so a new daemon must still speak the host's protocol. A daemon refuses an older host it cannot drive. |
| `zmux::pty_host::MINIMUM_HOST_PROTOCOL_VERSION` | `1` on Windows | `crates/zmux/src/pty_host.rs` | The oldest Windows host protocol a daemon is willing to drive | An upgrade is refused if the already-running host is too old. |
| `PROJECT_REGISTRY_VERSION` | `1` | `src/project.rs` | The local `projects.json` registry | A registry with another version is rejected; this affects project discovery and project handoff, but not Zetta-to-Zetta or Zetta-to-zmux communication. |
| performance report `schema_version` | `3` | `src/performance.rs` | JSON emitted by `zetta benchmark --profile-report` | Consumers of saved performance artifacts need a matching parser; it does not affect application startup or session compatibility. |

One additional value is a compatibility implementation detail rather than
the current version of a format:

- `MINIMUM_HOST_PROTOCOL_VERSION` is a compatibility floor, not a second
  independently negotiated current host format. It is listed above because a
  Windows upgrade can fail on that comparison.

There is also one deliberately versioned local cache artifact: macOS built-in
notification audio is stored with a `-v1.wav` suffix in
`src/notification_sounds.rs`. Bump that suffix when the waveform or WAV
encoding changes; it only invalidates regenerated audio and does not affect
Zetta peers, sessions, or persisted configuration.

## What does not belong in `zetta -v`

The repository also contains versions that come from another system or are
not Zetta compatibility contracts: dependency package versions, operating
system and graphics API versions, installed theme extension versions, and
the `max_schema_version=1` query sent to the external theme-extension index.
Those values can matter to the relevant subsystem, but adding them to the
application version block would make it look as if they identify a Zetta
peer or persisted file format. The executable's package version is already
propagated to `TERM_PROGRAM_VERSION`, the performance report, and the macOS
bundle metadata where appropriate.

When changing a versioned contract, update its constant and the relevant
sidecar/integration tests together. The catalog design plan currently mentions
an intended catalog version of 5, while the implementation publishes version
1; that discrepancy should be resolved explicitly before a future catalog
schema change rather than silently treating the plan as a protocol authority.
