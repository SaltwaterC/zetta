# Background sessions

Zetta can detach a complete tab while keeping its terminal processes and
scrollback alive. Detached sessions can be reconnected from any Zetta window in
the same process and can survive after the last window closes.

## Detach and reconnect

Use `Alt-Shift-D` or the archive button beside the new-tab button to detach the
active tab. Its rendered terminal views are destroyed, while a lightweight
background runner retains the live processes, scrollback, and complete tab
model, including:

- nested pane splits and the active pane
- minimized and maximized panes
- broadcast-input state
- pane and tab labels

Use `Alt-Shift-A` or the reconnect button to restore the only detached session
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
zetta sessions --json
```

The human-readable listing includes a stable `process:runner:session` ID, saved
split layout, active pane, profile, configured launch command, live foreground
application and full command line, terminal title, working directory, and
whether each pane is starting, running, exited, or failed.

`--json` provides the same catalog as structured, versioned JSON for scripts
and future remote-session tooling.

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
verifier. Hashing and verification run away from the UI thread.

## Automatically background a tab

Use `Alt-Shift-P`, the pin toggle in the tab bar, or **Zetta: Toggle Auto
Background Tab** in the command palette to keep a tab running when the tab or
its window closes.

Enabling the toggle requires choosing reattachment authentication immediately:
select **No authentication**, or enter and confirm a secret. Pinned tabs display
a pin in their label and move to the background automatically on close.
Unpinned tabs retain normal close behavior.
