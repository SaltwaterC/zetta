# Automatic updates: proposed design

Status: design only. No updater, release workflow, or installation changes
are implemented by this document.

## Recommendation

Build a Zetta-owned updater with Zed's interaction model: check in the
background, download and verify an update, then offer **Restart to update**.
Never restart automatically. Host the manifest and complete application
packages on GitHub Releases; a public release repository needs no separate
update service or client credential.

Start with one stable channel and one official feature set per supported
target. Source builds and package-manager installations remain outside the
self-update channel. Add preview releases and custom build variants only
after the stable installation and restart contracts are tested.

Use Zed's implementation as a reference, rather than importing its updater.
`zed/crates/auto_update` depends on Zed's client, database, settings and
workspace, and its Windows path assumes Zed's installer/helper layout.
Zetta has different packaging and terminal-session requirements. No upstream
files need to change.

## What exists today

- The root `Cargo.toml` defines the application version. `src/startup.rs`
  passes that package version to `release_channel::init`, keeping application
  state and CLI version output in sync.
- There is no root `.github/workflows` directory in this checkout. Release
  automation and the downloadable package contract must be added.
- Linux user installation puts the application and enabled helpers under
  `~/.local/zetta.app/bin`, with links under `~/.local/bin`. Root installation
  uses a different prefix.
- macOS installs a `Zetta.app` bundle. `scripts/bundle-macos.sh` currently
  applies an ad hoc signature; production distribution needs a signing and
  notarization step after the complete bundle is assembled.
- Windows ships `zetta.exe`, `zetta-gui.exe`, `wslx.exe`, optional helpers,
  and the ConPTY runtime. `scripts/install-windows.ps1` already stages files,
  retains locked generations, restores files on ordinary installation
  errors, and tracks the long-lived PTY host's install revision.
- `zmux` is a separately running daemon with an explicit upgrade protocol.
  Its compatibility markers and the Windows PTY host revision are documented
  in `versioning.md`.
- GPUI exposes restart hooks. Zetta's existing quit policy can retain
  sessions in the GUI process, so a plain application restart is not yet a
  session-preserving update operation.

These are observations about the local checkout, not an audit of any hosted
GitHub release or repository settings.

## GitHub Releases integration

Use this fixed discovery URL, assuming releases are published in the origin
repository:

```text
https://github.com/SaltwaterC/zetta/releases/latest/download/zetta-update.json
```

GitHub documents the
[`releases/latest/download/<asset-name>` URL](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases).
Attach `zetta-update.json` to every stable release, keeping that asset name
fixed. Every package referenced inside it uses a version-specific download
URL, never another `latest` URL. Publishing a new release during a download
then cannot change the package the client is verifying.

This design uses GitHub's asset delivery instead of polling its REST API.
The [Releases API](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)
is also viable: discover the stable release, compare `tag_name`, and use
the asset download URLs. However, unauthenticated REST calls have a
[60-request hourly limit per originating IP](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api),
shared by users behind the same network. The asset-based design avoids
consuming that REST budget; HTTP errors and service throttling still need
backoff.

For private source, publish distributable packages to a separate public
release repository if that fits the distribution policy. Private release
assets require authenticated access; they are not a credential-free
distribution channel. Never embed a shared GitHub token in the application.
An authenticated private channel would be separate work.

### Release assets and manifest

Each release contains the signed manifest and a complete package for each
advertised target. Example package names:

| Target | Example asset |
| --- | --- |
| Linux x86-64 | `zetta-0.2.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `zetta-0.2.0-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `zetta-0.2.0-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `zetta-0.2.0-x86_64-apple-darwin.tar.gz` |
| Windows x86-64 | `zetta-0.2.0-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `zetta-0.2.0-aarch64-pc-windows-msvc.zip` |

This is a naming proposal, not a claim that all six targets currently have
working release builds. Advertise only targets built and tested by release
automation. Record the minimum OS/runtime requirements, including the Linux
glibc baseline and included Wayland/X11 support. FreeBSD stays unsupported
by the initial updater unless an explicit package and test job are added.

Use a single signed envelope so fetching the manifest and signature cannot
race with publication of another latest release:

```json
{
  "key_id": "release-1",
  "payload": "<base64 of the exact UTF-8 manifest bytes>",
  "signature": "<base64 Ed25519 signature over the decoded payload bytes>"
}
```

The verified payload contains:

- Schema version, monotonically increasing release sequence, channel,
  package version, tag, source commit, publication time, and expiry time.
- Minimum updater version and a release-notes URL.
- A map of exact target triples and build variants to package URLs, byte
  lengths, SHA-256 hashes, minimum runtime requirements, and package layout
  versions.
- A complete owned-file inventory, including companion binaries and runtime
  files, plus the relevant daemon/PTY compatibility markers.

Embed trusted public verification keys in official builds. Keep the private
signing key in the release signing environment, available only to the
protected publishing job. Verify the signature before interpreting payload
URLs or extracting anything. A hash distributed alongside a binary detects
corruption but does not independently authenticate the publisher.

Parse and compare versions with `semver`; ignore build metadata for upgrade
ordering, never downgrade automatically, and reject a stable-channel payload
that advertises a prerelease. Persist the highest accepted release sequence
to reject previously seen older metadata. Expiry bounds replay for fresh
clients, subject to a reasonably accurate local clock; this is not complete
freeze-attack prevention. An expired manifest should leave the installed
application usable and explain why updates cannot proceed. Renew metadata
through a fresh release when necessary.

Rotate keys by first shipping an update trusted by the old key that adds
the new public key, before publishing updates signed only by the new key.
Loss of every trusted signing key requires a manual recovery release.

### Publishing sequence

1. Tag a version matching `Cargo.toml`; initialize submodules and build the
   official optimized packages with locked dependencies on native runners.
2. Assemble the entire payload, including separately built tools. Windows
   packages include both application entry points, WSL support, runtime files
   and the PTY revision marker. macOS packages contain the complete bundle.
3. Apply platform signatures and macOS notarization before hashing archives.
   Produce final package hashes, inventories and the signed manifest.
4. Run package validation and upgrade tests. Create a draft release, upload
   every asset, and verify the uploaded bytes before publication.
5. Publish only when the complete target set and manifest are present.
   Enable immutable releases where available. GitHub's
   [immutable-release workflow](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
   explicitly supports uploading all assets to a draft before publication.

Do not use automatically generated source archives as update packages.
Human installers and self-updates should consume the same assembled binary
payload. Every future stable release must carry the manifest, including a
release that does not otherwise change updater behavior.

## Application behavior and ownership

Introduce `src/auto_update/` with separate modules for the coordinator,
manifest verification, transport, installation planning, platform installers,
and restart preparation. Gate platform installers at their `mod`
declarations. Keep application behavior out of `main.rs` and avoid changing
the routing-only `release_channel` fork to implement Zetta update policy.

Create one updater entity per GUI process, initialized during startup and
observed by each window. Use the existing lazy HTTP client. Network access,
streaming hashes, extraction, signature checks and process waits run off the
UI thread. Fetch after startup with jitter, then approximately hourly; persist
the last check so repeated launches do not immediately fetch again. Apply
bounded timeouts, backoff, wake recovery, and cancellation to network work.
Do not initiate checks from short-lived CLI invocations.

Coordinate writers with a lock for the installation path, across processes.
A window must not begin a second update while another process is staging or
applying one. A completed pending update remains visible even if a later
check fails. Persist enough state to rediscover it after application exit.

The visible states are: idle/current, checking, downloading with progress,
verifying, ready to restart, applying, and failed with retry. Only report
success after validating the installed generation. Rate-limit progress
notifications so every network chunk does not invalidate the title bar.
Observe meaningful state changes from the existing chrome boundary; do not
cache an ancestor of terminal content.

Expose **Check for updates**, **Restart to update**, and **Release notes** in
the command palette and application menu, including the native macOS menu.
Keep action registration and keymap validation synchronized; resolve any
accelerator labels from effective bindings. Automatic failures are quiet
status indicators; manual checks report whether the app is current or why a
check failed.

Proposed typed configuration: `updates.mode` with `automatic`, `check_only`,
and `off`. Official installations default to `automatic`, which downloads
without restarting. `check_only` offers an explicit download action. `off`
disables background requests while allowing a manual check. Add configuration
schema, example, settings UI and parsing tests together. Initial channel
selection comes from installation metadata and remains stable-only.

## Installation contract

Record an installer-owned marker with the canonical installation root,
distribution owner, target, variant, installed version and layout version.
Official release installations opt into self-update. Writability alone is
insufficient: a writable source build, development checkout or managed
package must not silently become an official release installation.

An unmarked existing installation needs an explicit migration through the
release installer. Package-managed or protected system installations show
release availability with instructions for their owning installer. Do not
request elevation or replace package-manager files from the background
updater.

Download into a private staging directory. Enforce size limits and exact
hashes; reject traversal paths, escaping links, duplicate paths and unexpected
file types. Accept only HTTPS download locations allowed by the distribution
policy, while supporting GitHub's HTTPS asset-storage redirects. Never
forward credentials across origins or execute commands supplied by manifest
fields. Check layout, complete file inventory, architecture and available
disk space before allowing activation.

Stage the complete generation on the destination filesystem before applying
it. Use an installation lock and durable transaction journal: an exception
handler alone cannot recover from process termination or power loss. Preserve
the previous generation until successful startup acknowledgment. Updates do
not modify user settings, projects, shell startup files or user data.

| Platform | Activation design |
| --- | --- |
| Linux | Stage the complete application directory beside the existing one, then activate using a journaled directory replacement. Preserve public CLI links and desktop integration. Do not overwrite running executables in place. Account explicitly for any file capabilities; do not silently grant or assume privileged network access. |
| macOS | Stage and validate the entire signed/notarized app bundle beside the current bundle; preserve its identity and location. A helper outside the replaced bundle completes replacement and relaunch. Keep CLI wrappers resolving to the stable bundle path. |
| Windows | Reuse or extract the existing installer's file inventory, locked-generation, rollback and PTY-revision behavior. Add durable recovery and a package layout independent of a source checkout. A helper outside replaced files applies the staged payload after restart preparation and relaunches the GUI entry point. |

The Windows installer currently derives some inputs from repository-relative
paths and changes PATH during binary installation. Adapt those assumptions
for a packaged update instead of launching the script unchanged. If installer
code is packaged, authenticate it as part of the signed payload and invoke
only its fixed local entry point with structured arguments.

An apply helper must outlive the GUI and remain available if the old bundle
is moved. Do not perform installation work inside GPUI's short quit-hook
timeout. Persist the pending transaction and establish helper readiness before
exiting; the helper performs the filesystem work after the required processes
have exited. Wait for specific process identities, with a timeout, rather than
killing processes by name.

## Restart and terminal sessions

Treat restart preparation as its own phase, before activation:

1. Inspect all windows and dormant runners in the process. Classify panes by
   whether their processes and transports are actually hosted outside the GUI.
2. Serialize enough UI state to restore windows, tabs, pane layouts, focus and
   reconnect identifiers. A daemon retaining a PTY does not itself restore
   this state. Reuse existing session models where they fit.
3. Preflight the replacement daemon's handover compatibility against any
   running daemon before changing installed files. A manifest's advertised
   version markers supplement, rather than replace, runtime negotiation.
4. Refuse seamless restart for panes that cannot survive it. Offer deferral,
   or an explicit confirmation describing the sessions that would close.
   This includes `--no-mux`/no-`zmux` builds and any in-process transports or
   services not yet migrated. Do not claim all mux-enabled panes are safe.
5. Persist the restart plan, finish ordinary application cleanup, and let the
   helper activate, perform the supported daemon handover, and launch the new
   application through the intended new-window/resume path. Ordinary process
   handoff must not send the restart request to an unrelated old GUI instance.
6. Acknowledge successful startup and session restoration. Retain old locked
   runtime generations while any process still needs them.

Coordinate multiple GUI processes sharing an installation and daemon. Defer
activation if an older client would become incompatible with the replacement
daemon; do not silently break another window or terminate its processes.
If an old Windows PTY host cannot support the new daemon, defer the update
until its sessions close or use an explicitly tested compatibility path.

Filesystem rollback and daemon rollback are different operations. Before
daemon handover, a failed file transaction restores the old generation.
After handover, do not blindly restart old binaries: protocol and durable
state compatibility must be established first. Record that boundary in the
journal and provide a recoverable error when a safe automatic reversal is
not possible. Automatic rollback must never erase session state.

## Implementation sequence and acceptance

1. **Release contract:** package version initialization, installation marker,
   reproducible payload assembly, signing and draft-release publication.
   Validate packages without installing into the developer's live prefix.
2. **Discovery and verification:** signed-manifest parser, exact target
   selection, streaming download, typed update settings, coordinator and
   check/download UI. Test using fixtures and a fake HTTP client.
3. **Activation:** platform helpers, installation lock, journal/recovery and
   native upgrade tests against disposable installation roots.
4. **Restart:** window-state capture, daemon preflight/handover, session
   restoration, handling incompatible clients and explicit fallback for panes
   that cannot survive restart.
5. **End-to-end release exercise:** install an older test release, publish a
   newer release to a test repository, update through the UI, and verify app,
   helper versions and running sessions on each shipped OS/architecture.

Cover equal/older/prerelease versions, unknown schema/key/target, invalid
signatures, wrong hashes, truncated and oversized responses, interrupted
downloads, expired/replayed metadata, malicious archives, insufficient space,
read-only installs, failed launches and key rotation. Interrupt activation
between journal phases and verify deterministic recovery. Exercise concurrent
updaters, old GUI clients, live local and remote panes, mux-disabled builds,
Windows locked generations/PTY revisions, and macOS bundle identity.

Put application tests in matching `src/tests/auto_update/` sidecars. Extend
the existing Windows installer suites when sharing its behavior, and add
GPUI tests for state/action visibility without spawning real user shells.
Run root formatting, checks, tests, feature/platform checks and corresponding
clippy targets as required by `AGENTS.md`. Run native installation/restart
tests as well: cross-compilation cannot validate OS replacement behavior.

The first updater must be installed through the release installer once.
Later published release assets can then drive the full automatic update
cycle. The feature is complete only when packaging, activation, recovery and
restart have passed together; discovery alone is an intermediate milestone.
