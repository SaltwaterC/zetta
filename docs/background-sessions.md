# Background sessions

Zetta can detach a complete tab while keeping its terminal processes and
scrollback alive. Detached sessions can be reconnected from any Zetta window in
the same process and can survive after the last window closes.

## Detach and reconnect

Use `Ctrl-Shift-D` or the archive button beside the new-tab button to detach the
active tab. Its rendered terminal views are destroyed, while a lightweight
background runner retains the live processes, scrollback, and complete tab
model, including:

- nested pane splits and the active pane
- minimized and maximized panes
- broadcast-input state
- pane and tab labels

Use `Ctrl-Shift-A` or the reconnect button to restore the only detached session
immediately. When multiple sessions exist, the same control opens a picker.
Select by title, ID, pane count, or foreground application with the arrow keys
and `Enter`, or use the pointer.

The picker includes sessions detached from all Zetta windows in the process,
so a session may be detached in one window and attached in another. Detaching
the final visible tab creates a fresh tab so the window remains usable.

## Inspect sessions from the command line

Inspect detached sessions without opening another window:

```sh
zetta sessions
zetta sessions -j # or --json
```

The human-readable listing includes a stable `process:runner:session` ID, saved
split layout, active pane, profile, configured launch command, live foreground
application and full command line, terminal title, working directory, and
whether each pane is starting, running, exited, or failed.

`--json` provides the same catalog as structured, versioned JSON for scripts
and future remote-session tooling.

Reconnect a session by its stable ID:

```sh
zetta sessions reconnect 12345:7:42
```

Use the complete `PROCESS:RUNNER:SESSION` ID when more than one process has a
session with the same numeric ID. Reconnecting a protected session prompts for
the secret on the controlling terminal with terminal echo disabled. The secret
is read from the prompt rather than a command-line option, so it is not stored
in shell history or exposed in the process list.

## Closing the last window

When the last Zetta window closes, detached sessions keep the original process
running as a non-rendering session runner. Visible tabs close normally and do
not become background sessions implicitly.

Launching plain `zetta` again contacts the runner over an authenticated local
AF_UNIX control socket, reopens its window, and makes preserved sessions
available through the reconnect action. Once every background session is
reconnected or closed, closing the last window terminates Zetta normally.

## Session protection

When detaching a tab, choose **No authentication** or enter and confirm a
session secret. Protection is per session: unprotected sessions reconnect
immediately, while protected sessions prompt for their secret.

Zetta stores only a uniquely salted Argon2id verifier in the live session
runner. Neither the secret nor verifier is written to `config.json`, control
JSON, or the session catalog. Protected catalog entries expose only a stable ID
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
*channel*, not the session: no control command can reattach a protected
session, and none can observe or modify one. Renaming, attention, and
silent-mode queries all skip protected sessions, so the token cannot even be
used to confirm one exists.

The secret is never stored. Only a uniquely salted Argon2id verifier lives in
the session runner, and it is never written to `config.json`, control JSON, or
the session catalog. Editing files on disk cannot replace it. Protected catalog
entries carry only an ID and a protection flag, so commands, titles, and
working directories stay private while detached.

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
belong to the Zetta process. Any process able to read that process's memory or
open its file descriptors can talk to those terminals directly, without ever
presenting the secret. If the session runs a root shell, that is a privilege
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
palette to keep a tab running when the tab or its window closes. This
**Keep running** setting is separate from the visual `Pin Tab` action, which
only keeps a tab at the leading edge of the current tab bar.

Enabling the toggle requires choosing reattachment authentication immediately:
select **No authentication**, or enter and confirm a secret. Tabs with **Keep
running** enabled move to the background automatically on close; their tab-bar
pin indicator refers to that session policy, not to visual tab pinning.
