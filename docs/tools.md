# Serial and network tools

Zetta includes serial-console support, static HTTP and TFTP servers, a
command-line TFTP client, and desktop notifications. The servers have no
authentication or encryption; expose them only on networks whose clients you
trust.

Both servers listen on `0.0.0.0`, so they answer every host that can route to
the port, not only the local machine. This is deliberate: their purpose is
serving files to another device. There is no bind-address option, so restrict
access with a firewall when the network is not trusted. The **Start HTTP
server** and **Start TFTP server** actions do the same thing from the GUI, and
they serve the active pane's working directory — check which directory that is
before starting one.

These components are enabled in normal builds. Distribution builds can omit
them with `make build SERIAL=0 HTTP=0 TFTP=0 NOTIFY=0`; `TFTP_SERVER=0` and
`TFTP_CLIENT=0` select the two TFTP components separately. See the
[installation guide](installation.md#linux-desktop-integration) for the full
set of accepted flag values.

## Serial console

Choose **Zetta: Toggle Serial Console** from the command palette to enumerate
serial devices and connect one in a new left/right split.

The same console is available without starting the graphical application:

```sh
zetta serial list
zetta serial console --device /dev/ttyUSB0
zetta serial console --device /dev/ttyUSB0 --baud-rate 9600 --data-bits 7 --parity even --stop-bits 2 --flow-control hardware
```

`console` defaults to 115200 8N1 with no flow control. It uses raw terminal
input: `Ctrl-C` is sent to the device and `Ctrl-]` disconnects the local
console. `zetta serial list` prints one currently available device per line;
the shell integrations invoke it on each completion request, so devices plugged
in after `zetta init SHELL` are still offered for `--device`.

The dialog defaults to 115200 baud, 8 data bits, no parity, 1 stop bit, and no
flow control (115200 8N1). Use `Tab` to move between settings, arrow keys to
change the selected value, and `Ctrl-R`/`Cmd-R` to rescan. Baud rate, data bits,
parity, stop bits, and software or hardware flow control are configurable.

On Linux, placeholder legacy UART nodes are validated before display. When no
usable ports are detected, the dialog reports that no devices were found.
Closing the pane closes the device.

## TFTP server

Choose **Zetta: Start TFTP Server** from the command palette to serve files
below Zetta's launch directory on the configured UDP port, which defaults to
69. Zetta opens the server log in a new left/right pane, and each entry includes
a human-readable UTC timestamp. Press `Ctrl-C` in that pane, or close it, to
stop the server.

For an explicit command-line server, use:

```sh
zetta tftp server
zetta tftp server --root firmware --port 1069
zetta tftp server --config /path/to/config.json
```

The CLI server serves the current directory and uses `tftp_server_port` from
the selected configuration unless `--port` overrides it. It writes its request
log to standard output and stops on `Ctrl-C`. Set `tftp_server_port` in
`config.json` or use the **TFTP server port** setting; the value applies the
next time either server form starts.

Absolute paths, parent-directory traversal, and symlinks resolving outside the
served directory are rejected.

Uploads are refused unless `zetta tftp server --writable` (short form `-w`) is
given. TFTP has no authentication, so a writable server lets any host that can
reach the port create files under `--root`; keeping that opt-in means it cannot
happen by accident. Existing files are never overwritten, a single upload is
capped at 4 GiB, and incomplete uploads are removed after failed or cancelled
transfers. The GUI **Start TFTP server** action is always read only — use the
command line when you intend to receive files.

Binding port 69 may require privileges or firewall permission. On Linux, the
[installation guide](installation.md) explains how to grant the installed
binary only the required `cap_net_bind_service` capability. TFTP moves to a
dynamic UDP port after the initial request, so firewalls must permit related
transfer traffic.

Systems with a renamed loopback interface can explicitly allow local traffic,
for example:

```sh
sudo ufw allow in on loopback0 from 127.0.0.0/8 to 127.0.0.0/8
```

## HTTP server

Choose **Zetta: Start HTTP Server** to serve static files from Zetta's launch
directory on the configured TCP port, which defaults to 8000. For example:

```sh
wget http://HOST:8000/firmware.bin
```

The server supports read-only `GET` and `HEAD`, serves `index.html` when
present, generates a browsable index for other directories, and logs each
request in a new pane. Absolute paths, parent traversal, and symlinks resolving
outside the served directory are rejected. Press `Ctrl-C` in the server pane,
or close it, to stop the server.

The non-GUI equivalent makes its settings available as flags:

```sh
zetta http server
zetta http server --root firmware --port 8080
zetta http server --config /path/to/config.json
```

It serves the current directory by default and uses `http_server_port` from the
selected configuration unless `--port` overrides it. Request logs go to
standard output; `Ctrl-C` stops the server.

Set `http_server_port` in `config.json` or use the **HTTP server port** setting
to change the TCP port. The new value applies the next time the server starts.
Allow that port through the host firewall when necessary.

## TFTP client

Downloads default to the remote file's base name, uploads default to the local
file's base name, and `--port` targets a non-standard server port:

```sh
zetta tftp get HOST REMOTE [LOCAL]
zetta tftp put HOST LOCAL [REMOTE]
zetta tftp get --port 1069 HOST REMOTE [LOCAL]
```

The client uses octet mode and negotiates block-size and transfer-size options
when supported by the server. Run `zetta tftp --help` for complete syntax.
With [shell integration](shell-integration.md) enabled, `ztftp` is an
equivalent shortcut and retains TFTP command completion.

## Desktop notifications

`zntfy` is the standalone desktop notification command. It works from Zetta or
another terminal emulator and uses the current platform's native notification
system: D-Bus on Linux and BSD, Notification Center on macOS, and toast
notifications on Windows. With the `notifications` feature enabled,
`zetta notify` proxies to the sibling `zntfy` executable and accepts the same options.
Build both executables with `make build` for local development; packaged builds
install them together. To build them directly with Cargo, run
`cargo build --bin zetta` and
`cargo build --manifest-path crates/zntfy/Cargo.toml --target-dir target --bin zntfy`.

On macOS, `zntfy` installed inside `Zetta.app` submits through the app's main
executable. Notification Center therefore uses Zetta's authorized identity and
icon, and body clicks can route back to a tab. Native submission errors are
reported rather than silently switching to a generic script-host notification.
An unbundled standalone development build uses the script host and cannot
route clicks.

Silent mode is a transient process-wide control available from the title bar,
the command palette, and `Ctrl-Shift-S`. Tab Silent Mode is a separate
transient control available from each tab's context menu and the command
palette. Either one suppresses
terminal bells and every notification sound source for the affected tab while
leaving notification content, actions, and tab-attention badges unchanged. A
bell-off icon in the tab bar reports the tab's own setting; it does not mirror
process-wide silence or system Do Not Disturb.

Process-wide Silent mode's effective state is its manual setting or the
detected system Do Not Disturb state; Zetta observes system Do Not Disturb and
never changes it. Manual process-wide toggling is temporarily disabled while
system Do Not Disturb is active, then the previous manual preference resumes.
Notifications launched from a Zetta tab carry that tab's process-control
target, so its local Silent mode suppresses their sound without removing the
notification or its click action. Untargeted notifications continue to follow
the platform's system silence state.

On Windows, Zetta reads the shell's live Do Not Disturb profile every five
seconds. If that private system query is unavailable, fails, or returns an
unrecognized response, Zetta conservatively treats the system state as unknown
and immediately leaves manual Silent mode available instead of assuming that
Do Not Disturb is active.

On macOS, Focus status access is requested only when you choose
`Request Focus Status Access` in Settings or from the command palette. If access
is unavailable, denied, or restricted, Zetta leaves manual Silent mode
available and does not assume that the system is silent. Enable Zetta under
System Settings > Privacy & Security > Focus if you want Zetta to follow the
Focus status available to the app.

An `Authorized` permission does not guarantee that macOS supplies a live Focus
value. Apple also requires notification authorization and the Communication
Notifications capability for that value; the ad-hoc developer bundle does not
request that restricted capability, so Zetta reports live status as unavailable
and uses manual Silent mode.

Even when the live value is available, Apple exposes Zetta's app-specific
communication-notification status rather than a global DND switch. If Zetta is
allowed through the active Focus, macOS reports that Zetta is not focused.

```sh
zetta notify "Build finished"
zetta notify "Build finished" "All tests passed"
zetta notify --icon ./artifacts/logo.png --sound zetta-ok "Build finished"
zetta notify --sound zetta-alarm --timeout never "Long-running task complete"
zntfy "Build finished" "From any terminal"
```

SUMMARY is required and is the notification's title; BODY is optional
additional text. `--icon` takes a path to an image file, shown consistently on
every supported platform; without it, Zetta's own icon is shown. That icon is
bundled in the binary, so it is always available even without an installed
desktop entry. `--app-name` and `--timeout` (`default`, `never`, or a number of
milliseconds) behave the same everywhere except that some macOS notification
centers ignore the timeout and always show the application name as Zetta.
Run `zetta notify --help` for complete syntax.

`--sound` accepts `zetta-default`, `zetta-ok`, `zetta-alarm`, or `zetta-gong`:
tones that Zetta bundles and plays directly, so they sound the same
regardless of the host's sound theme, volume mixer routing quirks, or whether
one is configured at all. Any other value is passed through as a platform-specific
system sound name instead (for example a freedesktop sound-theme name such as
`message-new-instant` on Linux, a system sound name such as `Glass` on macOS,
or a toast sound identifier such as `IM` on Windows) and only plays if the
platform recognizes it. On macOS, Notification Center owns playback of system
sound names, while Zetta's built-in tones continue asynchronously after
`zetta notify` exits.

When `zntfy` or `zetta notify` runs inside a Zetta terminal with both inherited
`ZETTA_PROCESS_ID` and `ZETTA_ATTENTION_ID` values valid, clicking the
notification body activates the matching Zetta window and visible tab.
Dismissing, timing out, replying to, or choosing another notification action
does not focus anything. If the tab has closed or is only dormant/background,
the click is a no-op; Zetta does not reconnect sessions for notifications.
Outside Zetta, or when either inherited value is missing or invalid, the
notification remains fire-and-forget. Packaged macOS app builds route via the
main Zetta executable. Unbundled development builds display through the script
host without click routing.

With [shell integration](shell-integration.md) enabled, the `zntfy` shortcut
retains notification command completion.

A notification targeting a tab spawns a background `zntfy` process on platforms
where the notification client must wait for the click. On packaged macOS, the
running Zetta app handles the click instead. The command confirms native
submission and reports authorization or backend errors rather than claiming
success before submission. Focus and per-app alert settings can still suppress
a banner after submission succeeds. The worker exits once the notification's
own timeout has passed. On Linux and BSD, `zntfy cleanup` and
its `zetta notify cleanup` proxy find and terminate any workers that have outlived their
notification's timeout; `--dry-run` lists them without terminating anything.
This is normally unnecessary, since each process bounds its own lifetime to
the notification's timeout, but some notification servers (GNOME Shell in
particular) do not always signal a notification's expiry, which can leave one
of these processes running indefinitely with nothing left to click; run
`zetta notify cleanup` if you notice one. Run `zetta notify cleanup --help`
for complete syntax.

## Tab attention

`zetta attention` marks the tab that launched the command with a small,
theme-colored badge. The badge is held in memory and clears when that tab is
selected and its terminal (or minimized-pane shelf) genuinely receives focus.
It never falls back to whichever tab happens to be active: the command must
inherit the `ZETTA_PROCESS_ID` and `ZETTA_ATTENTION_ID` variables from a Zetta
terminal, and it reports an error if the originating tab has closed.

```sh
zetta attention
zetta attention "Build finished" "All tests passed"
zetta attention --notify --sound zetta-ok "Deploy finished"
```

SUMMARY defaults to `Attention required`; BODY is optional. `--notify` adds a
desktop notification using the same `--app-name`, `--icon`, `--sound`, and
`--timeout` options as `zetta notify`. Those notification options are rejected
unless `--notify` is present. Clicking the notification body focuses the
originating visible tab when it still exists; it does not reconnect a dormant
background session. Run `zetta attention --help` for complete syntax.

## Clipboard

`zcopy` reads standard input into the system clipboard, and `zpaste` writes
clipboard text to standard output. They are standalone executables; `zetta copy`
and `zetta paste` validate arguments and forward to them with inherited standard
streams. Their behavior mirrors macOS's
`pbcopy`/`pbpaste`:

```sh
echo "Build finished" | zcopy
zcopy < release-notes.txt
zpaste > release-notes-copy.txt
zetta paste | grep TODO
```

Both accept `-pboard {general | ruler | find | font}` for `pbcopy`/`pbpaste`
compatibility; Zetta has only one clipboard, so the value is validated but
otherwise has no effect. `zetta paste` also accepts `-Prefer {txt | rtf | ps}`
for the same reason: Zetta only ever stores plain UTF-8 text, so the
preference is validated but does not change the output. `zetta paste` prints
nothing, without an error, if the clipboard is empty or holds no text. Run
`zetta copy --help` or `zetta paste --help` for complete syntax.

In an interactive SSH or zosh pane, the helpers first probe the Zetta window
displaying that pane. `zcopy` then copies to that window's clipboard. `zpaste`
reads it only after **Allow Remote Clipboard Paste** is enabled in the tab
menu or command palette. The switch starts off and is cleared when the tab
closes or detaches. Copying is always permitted. A disabled read or a failed
transfer exits with an error; a channel that does not answer within three
seconds makes a helper with a local backend use that machine's clipboard.
Transfers stop after 30 seconds without a response. Clipboard text has no
protocol size limit; data travels in 32 KiB chunks through the controlling
terminal, separate from stdin and stdout, so pipelines work. A remote command
needs a TTY (`ssh -t` for a one-shot command).

On a headless remote host, build the helpers without a native clipboard
backend from a Zetta checkout:

```sh
cargo install --path crates/zclip --no-default-features --locked
```

Put the installed `zcopy` and `zpaste` on the remote `PATH`. A backend-free
helper reports an error if no Zetta channel answers. The Zetta host needs its
own native sibling helpers installed alongside the Zetta executable.

On Linux and BSD, the X11 and Wayland clipboards are only available while
their owning process keeps running, so `zcopy` starts a detached
background process that keeps serving the clipboard after the command exits.
macOS and Windows keep the clipboard through their own system services, so no
such process is needed there.

`zcopy` and `zpaste` run directly without shell integration. [Shell
integration](shell-integration.md) adds completions for them. On every platform
other than macOS (which already has real `pbcopy`/`pbpaste`), the integration
also defines `pbcopy` and `pbpaste` as functions calling the standalone tools, taking priority
over any preexisting `pbcopy`/`pbpaste` alias (for example one pointing at
`xclip`) so that muscle memory keeps working there too.
