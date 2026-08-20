# Background sessions

Zetta can detach a complete tab while keeping its terminal processes and
scrollback alive. In normal daemon mode, detached sessions can be reconnected
from any Zetta window and survive Zetta closing entirely. `zetta --no-mux` uses
the compatibility owner inside the Zetta process instead: background sessions
still work, but they are not shared with another process and end when that
Zetta process ends.

## Where a session lives

Terminal processes belong to `zmux`, a separate multiplexer that Zetta starts
the first time it needs one. That is what lets a session outlive the window it
was started in — and, in time, be attached from another machine.

While a pane is on screen, `zmux` hands Zetta the terminal itself rather than
copying its output, so an attached pane costs exactly what a locally spawned one
costs. When a tab is detached, Zetta gives the session back, along with a
snapshot of each pane's screen; `zmux` keeps reading the terminals so nothing
blocks, and replays the snapshot and everything since when the session is
attached again.

`zmux` is also reachable as `zetta mux`, and takes the same arguments either
way:

```sh
zmux list             # the sessions being held
zmux share SESSION_ID    # make a scoped session shared/joinable
zmux reconnect SESSION_ID # open it in a Zetta window
zmux unshare SESSION_ID  # scope it back to the window that held it
zmux kill SESSION_ID     # end a session and everything running in it
zmux stop             # stop the multiplexer, once nothing is running in it
zmux --upgrade        # replace the multiplexer, keeping its sessions
```

`SESSION_ID` accepts the short numeric session ID when it is unambiguous, and
the stable `PROCESS:RUNNER:SESSION` identifier printed by `zmux list` in every
case. The human-readable list shows both forms for an unambiguous session and
only the full form when numeric IDs conflict. Shell completion offers the
stable form for `share`, `reconnect`, `unshare`, `kill`, and `forget`.

`share` and `reconnect` are deliberately separate actions. `share` changes a
session's scope so any Zetta process may attach it; it does not open a window.
`reconnect` is the action that opens the session in a Zetta window. A scoped
session therefore needs `zmux share SESSION_ID` first, followed by `zmux
reconnect SESSION_ID`. The same commands are available as `zetta mux share`
and `zetta mux reconnect`.

Normal startup requires the multiplexer. If it cannot be started or reached,
Zetta reports the error and does not create a terminal outside daemon ownership.
Use `zetta --no-mux` only when the explicit compatibility path is wanted; it
keeps the legacy in-process runner for that launch, so those sessions end with
the Zetta process and are not held by the daemon. The in-process runner still
supports **Detach** and **Keep running**: closing a window moves those tabs into
the dormant Zetta process, and reopening a window in that same process can
reconnect them. `Keep running` does not imply sharing in this mode, and Zetta
does not inspect, connect to, or stop an already-running multiplexer. On the
Phase 0–2 Windows build, daemon mode is intentionally gated; start with
`zetta --no-mux` until the Windows pseudoconsole host lifecycle is delivered.
The standalone `zmux.exe` package is still built and installed for
daemon-capable platforms.

Debug builds use a protocol-scoped `sessions-debug-vN` directory. This lets a
`target/debug/zetta` and its adjacent `target/debug/zmux` run alongside an
installed release without sending development requests to the release daemon;
debug `zmux` commands operate on that same debug daemon.

### Stopping the multiplexer

`zmux stop` ends the multiplexer itself. It refuses while it is holding a
session, naming how many, because the daemon owns those terminals and stopping
it ends every process running in them; `zmux stop --force` says you meant that.
Stopping one that is not running is not an error, so it is safe in a script.

It is the answer to `pkill zmux`, which matches every multiplexer this user is
running — another checkout's, another test's — and takes their sessions with it.
`stop` acts on the multiplexer for one session directory: the one that answered,
whose process id it published for itself.

A multiplexer left over from an earlier build speaks a protocol this one cannot,
and `--upgrade` is the only request that crosses that boundary — so it cannot be
*asked* to leave. That is the other case `--force` covers: it is signalled,
`SIGTERM` first and `SIGKILL` only if that is ignored. `zmux --upgrade && zmux
stop` is the gentler route, and the one to prefer when it is holding sessions:
after the upgrade both ends agree about the protocol, so the stop is an ordinary
request again. Either way `stop` waits for its socket to go quiet before reporting
success, because a reply says the request arrived, not that the process left.

### Replacing the multiplexer

`zmux --upgrade` replaces the running multiplexer without ending anything it is
holding. It re-executes itself, so the process, its terminals and its
parent/child relationship with every shell all survive; a session's protection
and its failed-attempt backoff survive with them. The replacement is checked
before it is run, so a multiplexer that cannot be replaced keeps running rather
than taking its sessions down with it.

Panes that are on screen at the time are unaffected, whether or not they were
ever detached. Zetta holds their terminals itself, so they keep displaying output
and accepting input straight through the replacement. What does not survive is
the control connection each window keeps for events such as a pane's process
ending: an `execv` cannot carry a socket. The multiplexer announces the
replacement first, each window reconnects through the same socket path and token,
and on reconnecting asks what it missed — so an exit that happened during the
changeover is still reported, with its real status.

A window only reports losing the multiplexer once it has failed to reach one for
a full minute. Until then the panes are simply running, because nothing about
their terminals has changed.

`--upgrade` is also the one command that works across a protocol version
boundary, because crossing one is exactly what it is for. Rebuilding Zetta leaves
a new client and an old multiplexer; every other request between the two is
refused with a version error, but the upgrade goes through, and after it both
speak the new protocol. That takes agreement at both ends: the multiplexer exempts
the request from the version check it applies to everything else, and the client
connects without insisting on a version it is about to change. Missing either half
makes the command report the mismatch it exists to resolve — which is what it did
until the client's half was there. The replacement is still the image the
multiplexer resolved for itself at startup — never one a client names — so this is
no more privileged than any other upgrade.

Upgrading is not supported on Windows, where a pseudoconsole cannot be moved
between processes. Phase 0–2 Windows therefore uses the explicit `--no-mux`
path; sessions have to be closed there before the daemon lifecycle is enabled.

### How much output is kept

A detached pane is always read — otherwise its program would block once the
terminal's buffer filled — but how much is *kept* is configurable:

| `sessions.retention` | what a reattached pane shows |
| --- | --- |
| `memory` (default) | the screen as it was, plus output since, up to `sessions.ring_bytes` |
| `none` | a cleared screen and whatever the program redraws |

`none` is for hosts where memory matters more than scrollback. The
`scrollback-buffer` feature compiles the buffer out altogether.

What is kept is a bounded *terminal grid*, rather than raw bytes: the multiplexer runs an off-screen
terminal for every pane it reads, feeds it the same output the pane produces, and
serializes the screen from it when a window asks for the session. Keeping bytes
instead could not work for a full-screen program, whose output repaints parts of a
screen — a bounded buffer of its recent output is a pile of fragments describing a
screen the buffer has already dropped to make room for them, which is why a
reattached `htop` came back as pieces of itself over a blank terminal. A grid is
what a terminal keeps for the same reason, and it holds the screen however long
the session runs.

`sessions.ring_bytes` is therefore a scrollback budget: the screen
plus a bounded history above it, with the oldest lines going first, exactly as a
terminal's own scrollback does. Reading every byte costs the multiplexer roughly
what it costs a terminal — a pane producing output at full tilt while nobody is
showing it drains about a third slower than one whose output is discarded — and it
costs nothing for a pane a window is reading itself, which the multiplexer does
not read at all.

### Whose a session is

A backgrounded session belongs to the Zetta process that backgrounded it. No
other window lists it in its reconnect picker, and the multiplexer refuses an
attach from anywhere else — which is what backgrounding meant before the
multiplexer held these sessions at all. `Ctrl-Shift-D` is not a way to hand a
tab to another window; `Ctrl-Shift-K` is.

Sharing is the separate, explicit request, and it works on a session in the
background as well as one on screen:

```sh
zmux share 12345:7:3      # make it joinable, with its secret
zmux reconnect 12345:7:3  # open it in a Zetta window
zmux unshare 12345:7:3   # the window that last held it owns it again
```

`Ctrl-Shift-K` asks for the secret a joining window will have to present, exactly
as detaching asks for the one that will reattach a session — and, exactly as
there, an empty dialog leaves the session unprotected. It is worth choosing one:
a window joining a session can do everything that session's terminals can
already do, and that may be more than the joining process could do on its own,
since a shell that has answered `sudo` stays answered. `zmux share` offers the
same choice at the terminal.

A tab that already carries a secret — one configured for keeping it running — is
shared with that secret rather than being asked for another, and a session the
multiplexer has already protected keeps the secret it has. Scoping a session back
needs no secret and leaves the one it has in place.

`unshare` scopes a session back to the process that last held it, not to the
command that asked — a CLI is a process that exits a moment later. It therefore
needs that window to still be running, and it refuses a session that is on
screen, because a session several windows are driving may only be scoped back
from the one showing it, which is what `Ctrl-Shift-K` does there.

The scope outlives the window in normal daemon mode. A session detached with
`Ctrl-Shift-D` whose Zetta has exited — closed or crashed — stays that process's,
and no other window may attach it: ordinary detaching is not a slow way of
sharing it. `Ctrl-Shift-B` is the exception: **Keep running** also shares the
session, so the handoff is available to a new Zetta process after the window
closes. `zmux list` says which process each still-scoped session belongs to. If
that process is gone, run `zmux share SESSION` to change the scope, then
`zmux reconnect SESSION` to open it. It can also be ended with `zmux kill SESSION`
by the session owner or current holder, like any other protected administrative
action.

With `zetta --no-mux`, the same **Detach** and **Keep running** actions retain
the tab in the current Zetta process. Closing its last window leaves that
process dormant while the session exists; open a new window in that process to
reconnect it. There is no shared live-pane attach or daemon reconnect path, and
ending the Zetta process ends the locally owned sessions.

## Detach and reconnect

Use `Ctrl-Shift-D` or the archive button beside the new-tab button to detach the
active tab. Its rendered terminal views are destroyed, while `zmux` retains the
live processes, bounded screen/scrollback, and complete tab model, including:

- nested pane splits and the active pane
- minimized and maximized panes
- broadcast-input state
- pane and tab labels

Use `Ctrl-Shift-A` or the reconnect button to restore the only detached session
immediately. When multiple sessions exist, the same control opens a picker.
Select by title, ID, pane count, or foreground application with the arrow keys
and `Enter`, or use the pointer.

The picker includes sessions detached from all Zetta windows in the process,
so a session may be detached in one window and attached in another — and
sessions shared by other processes, which is what makes them attachable here.
It does not include another process's own backgrounded sessions; see "Whose a
session is" above. Detaching the final visible tab creates a fresh tab so the
window remains usable.

## Sharing a tab between windows

Sharing is its own workflow, not a step inside detaching. Use `Ctrl-Shift-K`,
**Share Tab** in the tab's context menu, or **Zetta: Toggle Tab Sharing** in the
command palette. The tab stays exactly where it is — same window, same panes,
still reading its own terminals — and the session becomes visible to other Zetta
windows, which is all that was missing.

From there:

1. Share the tab in the first window, choosing a secret or **No authentication**
   when asked. It now appears in `zmux list` and every
   window's reconnect picker, marked **in use** because a window is still
   showing it.
2. Reconnect that session from a *second* Zetta process, entering the secret if it
   has one. The multiplexer asks the first window to hand its terminals over; it
   snapshots each screen and rejoins — without being asked for the secret, because
   it is answering a handover it was sent, not joining a session — and from then
   on both windows drive the same panes.
3. Anything typed in either window goes to the same shell, and both see all of
   the output.

It has to be a second *process*, not a second window in the same one. The
multiplexer identifies a client by its process, so it recognises a second window
of the same Zetta as the holder re-attaching and hands the pane straight back
exclusively. Plain `zetta` hands off to the window that is already running; pass
`--profile NAME` or `--config PATH` to get an independent process against the
same multiplexer. Attempting it in the window that shared the tab is refused with
a message rather than silently opening a second tab onto the same terminal.

The screen handed over in step 2 is what every window that joins later starts
from, however long after the share it arrives — the multiplexer keeps it, along
with what the pane has printed since, rather than spending it on the first window
to join. The window that handed it over is the exception: it never stopped
showing that screen, so it is sent only what arrived afterwards. A full-screen
program repaints just what it thinks has changed, so a window sent the wrong one
of those two would keep whatever it never repainted — htop's F-key bar drawn
across the top of the window, or a screen with its static text missing.

While a pane is shared, every viewer shows it at the smallest size any of them
reports, per axis, so a small window shrinks the pane for everyone. A pane's exit
also records whether any viewer typed into it, which only the multiplexer can
know once input is arriving from more than one place.

When the viewers come back down to one, the multiplexer hands the terminal back:
the remaining window reads the pane directly again and nothing is relayed. That
happens by itself, keeps the pane's screen and scrollback exactly as they are, and
restores full throughput — a relayed pane runs at about a quarter less than one its
window reads itself. Sharing the tab again, or a third window joining, hands it the
other way, so a session can move between the two modes as often as its viewers
change.

Sharing does change how a pane's output reaches you. An exclusive pane is read
straight from its terminal by the window showing it; a shared one is read by the
multiplexer and relayed to each viewer over its socket, so a keystroke's echo
makes two extra hops. That costs tens of microseconds, not milliseconds — the
multiplexer waits on the terminal rather than polling it on a timer, which is
what keeps the difference below what anyone can perceive.

`Ctrl-Shift-K` is a toggle, and switching it off scopes the session back to this
window: it stops being listed and attachable, and its panes stop being relayed —
this window reads its own terminals again.

That is only possible while **one** window has the session, so unsharing is
refused while another window is still viewing it, saying how many are. There is no
way to take a pane away from a chosen viewer: the multiplexer hands one back to the
*last* one left, so the others have to close their tab first. The tab stays shared
until then, which is what the context menu's checkmark shows.

A viewer slower than the program it is showing is waited for, not dropped: the
program is throttled to the rate the slowest viewer can render, exactly as it is
throttled by a single window reading its own terminal. That includes a viewer that
pauses for a moment — a window laying out a full-screen redraw stops reading as a
matter of course, and it is waited for too.

What releases a pane is a viewer *going*: closing its tab, or its process ending.
There is also a last-resort timeout of half a minute for a window that has hung with
a pane still attached, so one frozen window cannot hold a session for ever.

Sharing and **Keep running** remain separate properties in daemon mode, but
enabling **Keep running** requests both: the tab outlives this window and its
session is shared by default. Sharing alone does not keep a session alive after
every viewer goes, so a shared tab whose window closes without **Keep running**
ends like any other. In `--no-mux` mode, **Keep running** only requests the
process-local background owner because sharing is unavailable.
If sharing is explicitly turned off before a keep-running tab closes, the
handoff is private again. The authentication dialog covers both properties: a
tab that already carries a secret reuses it, and a tab without one is asked.
Worth answering rather than skipping — "running as you" is not the boundary that
matters here, because a session's terminals may hold privileges the process
joining them does not.

## Inspect sessions from the command line

Inspect detached sessions without opening another window:

```sh
zetta mux list
zetta mux list -j # or --json
```

The human-readable listing includes a stable `process:runner:session` ID, saved
split layout, active pane, profile, configured launch command, live foreground
application and full command line, terminal title, working directory, and
whether each pane is starting, running, exited, or failed. A failed pane also
has an `exit:` line describing why Zetta retained it, with an exit code and
child PID when those values are available.

`--json` provides the same catalog as structured, versioned JSON for scripts
and future remote-session tooling. Failed panes include structured exit metadata
such as the source of the report, the classification, and the sanitized
foreground command name. Catalogs written by older schema versions are ignored
until their owning process publishes the current format.

Reconnect a session by its stable ID. Use `share` first if the listing says it
is scoped to another process:

```sh
zetta mux share 12345:7:42
zetta mux reconnect 12345:7:42
# `zmux share` and `zmux reconnect` are equivalent standalone commands.
```

Use the complete `PROCESS:RUNNER:SESSION` ID when more than one process has a
session with the same numeric ID. Reconnecting a protected session prompts for
the secret on the controlling terminal with terminal echo disabled. The secret
is read from the prompt rather than a command-line option, so it is not stored
in shell history or exposed in the process list. When this command runs inside
a Zetta terminal, the reconnect is routed to that terminal's Zetta process and
window; an invocation from outside Zetta uses the available running window.

In a shell launched by `zetta --no-mux`, the local session catalog is still
managed from the CLI:

```sh
zetta mux list
zetta mux reconnect PROCESS:RUNNER:SESSION
```

`share`, `unshare`, `kill`, `forget`, `stop`, and `--upgrade` require a daemon,
so `zetta mux --help` and shell completion omit them in that mode. The same
filter applies to the standalone `zmux` command. A session kept in this mode
remains owned by that Zetta process and cannot be shared with another process.

## Unexpected terminal exits

Zetta closes an interactive pane automatically when its shell reports an
ordinary user-initiated exit. A clean exit status of zero is always treated
as an ordinary close, even when process metadata is stale or missing: the
foreground check only flags a command that is still running, and a pid that
no longer exists counts as unknown rather than a failure. If the exit status
cannot be obtained, the child watcher or terminal backend disconnects, the
shell exits before receiving user input, or process metadata shows a command
such as `htop` still in the foreground, Zetta treats the exit as unexpected.

Unexpected exits are retained instead of silently removing the pane. The pane
shows a **Terminal exited unexpectedly** message and keeps its tab and split
layout, so the failure can be inspected or the other panes can continue to be
used. The same pane is marked `failed` in a detached session's catalog.

Losing contact with the multiplexer is retained the same way but reads
differently — **Lost contact with the session multiplexer** — because it is not
the same thing. The pane's process may well still be running, and the session may
still be attachable from another window; what has been lost is Zetta's ability to
be told how that process ends. Only the multiplexer is the process's parent, so
no other route exists. This is deliberately not the message a replaced
multiplexer produces: see "Replacing the multiplexer" above.

A pane whose terminal has hung up with no report from the multiplexer within a
few seconds is reported as having exited with an unavailable status, rather than
waiting indefinitely. Without that bound such a pane could not be closed at all.

Reconnecting a failed session restores the tab and displays the diagnostic pane;
healthy panes in the same split remain available. Closing that tab dismisses the
retained failed session rather than moving it into another background session.
The retained `exit` metadata and failure log context contain only structured
exit metadata and a sanitized command name. They do not copy terminal output,
environment values, or a full command line. The catalog's existing foreground
`command line` field remains separate and may still be present when that
metadata was available.

## What a detached tab keeps

A detached tab comes back with its layout, labels, overlays, icon, pinning and
per-pane configuration, and each pane's screen as it was.

Commands *stacked* on a pane — run in front of its shell rather than in it — come
back listed with their recorded outcome. A command that was still running is
stopped before detach, its daemon pane is closed, and it is restored as a failed
task rather than shown as running: a stacked command's terminal cannot yet be
reattached, so restoring it as running would leave something that never finishes.

## Closing a pane

Closing a pane whose program is still running tells the multiplexer to take the
terminal back. That matters because Zetta dropping its own descriptor says
nothing: the multiplexer holds one too, so without being told it would keep the
pane marked as taken, read nothing from it, and the program would block as soon
as the terminal's buffer filled. If the pane was the last one in a session nobody
asked to keep, the session ends with it.

## Closing the last window

Detached sessions belong to the multiplexer, so closing every Zetta window — or
Zetta exiting unexpectedly — leaves them running. Visible tabs close normally
and do not become background sessions implicitly.

Launching `zetta` again finds the multiplexer through an authenticated local
AF_UNIX control socket and offers its sessions through the reconnect action.
`zmux list` (or `zetta mux list`) lists them without opening a window at all,
because the multiplexer publishes the catalog to disk. Use `zmux reconnect` or
`zetta mux reconnect` when you want to open one.

## Session protection

Every action that makes a session reachable beyond the window driving it asks the
same question, in the same dialog: detaching a tab (`Ctrl-Shift-D`), keeping it
running after close (`Ctrl-Shift-B`) and sharing it (`Ctrl-Shift-K`). Choose **No
authentication** — or press Enter with both fields empty — or enter and confirm a
secret.

Protection is per session: unprotected sessions reconnect immediately, while
protected sessions prompt for their secret. It is worth choosing one for a
session whose terminals can do more than whatever might pick it up later, because
what attaches a session gets everything those terminals can already do.

A session that already has a secret is not asked for another: the one it has
stands, whether this window chose it or the multiplexer has been holding it since
before this window attached.

Only a uniquely salted Argon2id verifier is stored, in the multiplexer holding
the session. Zetta hashes the secret and sends only the result, so the secret
itself never crosses the socket. Neither the secret nor the verifier is written
to `config.json`, control JSON, or the session catalog. Protected catalog entries expose only a stable ID
and protection flag, so commands, titles, and working directories remain
private. Editing catalog or configuration files cannot replace the live
verifier. Hashing and verification run away from the UI thread, and a failed
attempt is answered after a fixed delay so a weak secret cannot be guessed at
speed.

### What session protection covers

Session protection is intended as a real boundary, including for a session
running a privileged shell. Three properties hold it up.

Reattaching a protected session requires the secret. The process control socket
is mode `0600` in a `0700` directory, and its endpoint token authenticates the
*channel*, not the session. Protected catalog entries reveal only a stable ID
and protection flag; direct pane-state observation and administrative commands
(`kill`, `forget`, `resize`, `close`, and scope changes) require the session
owner or a current holder. On Linux the daemon binds that decision to the
Unix-socket peer PID, so changing only the JSON process-ID field is not enough.
Renaming, attention, and silent-mode queries also skip protected sessions.

The secret is never stored. Only a uniquely salted Argon2id verifier lives in
the `zmux` daemon's memory during Phase 0–2, and it is never written to
`config.json`, control JSON, or the session catalog. Phase 3 may place the
verifier inside an age-encrypted record, never in cleartext. Editing files on
disk cannot replace it. Protected catalog entries carry only an ID and a
protection flag, so commands, titles, and working directories stay private
while detached.

Wrong secrets are rate limited with an escalating backoff. Each consecutive
failure doubles the window during which that session refuses further attempts,
from one second up to thirty, and a correct secret resets it. Attempts arriving
inside the window are refused without being evaluated and report the same
failure as a wrong secret, so the window cannot be probed. Backoff is per
session, so guessing at one session neither locks you out of another nor
accumulates into shared state. Attempts also serialize through the control
socket, making this a global bound on the guessing rate rather than a
per-connection one.

### The prerequisite: process memory must be protected

There is one assumption underneath all of the above, and it is worth checking
rather than assuming.

A detached session's terminals are ordinary PTYs whose master file descriptors
belong to the `zmux` process. Any process able to read that process's memory or
open its file descriptors can talk to those terminals directly, without ever
presenting the secret. The same is true of Zetta itself while a session is
attached, since it is holding those descriptors then. If the session runs a root shell, that is a privilege
escalation, and no amount of authentication inside Zetta can prevent it.

On Linux, the setting that governs this is the Yama LSM:

```sh
sysctl kernel.yama.ptrace_scope
```

A value of `1` or higher is required. `1` (the default on Debian, Ubuntu, and
most derivatives) restricts `ptrace` to descendants, which also gates
`/proc/<pid>/fd`, so an unrelated process running as your user cannot reach
Zetta's terminals. A value of `0` — set by some distributions and by developers
who need unrestricted debuggers — removes that restriction and voids session
protection entirely against any code running as your user. Values of `2` or `3`
are stronger still.

macOS restricts `task_for_pid` to root or specially entitled processes by
default, which provides the equivalent guarantee. Windows requires
`SeDebugPrivilege` or matching ownership to open a process for memory access.

Two things remain outside the boundary on every platform. Root can always read
any process, so protection is against other unprivileged code, not against a
compromised superuser. And a privileged shell survives detachment as a running
process: if you would not leave it running unattended in a `screen` or `tmux`
session, session protection does not change that calculation — it narrows who
can pick it back up.

## Automatically background a tab

Use `Ctrl-Shift-B` or **Zetta: Toggle Auto Background Tab** in the command
palette to keep a tab running when the tab or its window closes. **Keep running**
also shares the session by default in daemon mode, so a new Zetta process can
reconnect after the handoff. With `--no-mux`, it keeps the session in the same
Zetta process without sharing. This setting is separate from the visual `Pin
Tab` action, which only keeps a tab at the leading edge of the current tab bar.

Enabling the toggle asks for reattachment authentication immediately, in the same
dialog detaching and sharing use: select **No authentication**, or enter and
confirm a secret. A session that already has one is not asked again. Tabs with
**Keep running** enabled move to the background automatically on close; their
tab-bar pin indicator refers to that session policy, not to visual tab pinning.
