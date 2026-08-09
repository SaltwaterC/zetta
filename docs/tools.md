# Serial and network tools

Zetta includes serial-console support, static HTTP and TFTP servers, a
command-line TFTP client, and desktop notifications. The servers have no
authentication or encryption; expose them only on networks whose clients you
trust.

These components are enabled in normal builds. Distribution builds can omit
them with `make build SERIAL=0 HTTP=0 TFTP=0 NOTIFY=0`; `TFTP_SERVER=0` and
`TFTP_CLIENT=0` select the two TFTP components separately. See the
[installation guide](installation.md#linux-desktop-integration) for the full
set of accepted flag values.

## Serial console

Press `Ctrl-Shift-S` or choose **Zetta: Toggle Serial Console** from the command
palette to enumerate serial devices and connect one in a new left/right split.

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
served directory are rejected. Uploads may create files below that directory,
but never overwrite existing files. Incomplete uploads are removed after failed
or cancelled transfers.

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

`zetta notify` shows a desktop notification through the native notification
system of the current platform: D-Bus on Linux and BSD, Notification Center on
macOS, and toast notifications on Windows.

On macOS, an installed CLI transparently submits notifications through the
signed `Zetta.app` bundle and the modern `UNUserNotificationCenter` API. The
first invocation asks macOS for notification permission. Standalone development
builds fall back to macOS's bundled script host, so they do not require an app
bundle merely to show a notification.

```sh
zetta notify "Build finished"
zetta notify "Build finished" "All tests passed"
zetta notify --icon ./artifacts/logo.png --sound zetta-ok "Build finished"
zetta notify --sound zetta-alarm --timeout never "Long-running task complete"
```

SUMMARY is required and is the notification's title; BODY is optional
additional text. `--icon` takes a path to an image file, shown consistently on
every supported platform; without it, Zetta's own icon is shown. That icon is
bundled in the binary, so it is always available even without an installed
desktop entry. `--app-name` and `--timeout` (`default`, `never`, or a number of
milliseconds) behave the same everywhere except that some macOS notification
centers ignore the timeout and always show the application name as Zetta.
Run `zetta notify --help` for complete syntax.

`--sound` accepts `zetta-default`, `zetta-ok`, or `zetta-alarm`: short tones
that Zetta synthesizes and plays itself, so they sound the same regardless of
the host's sound theme, volume mixer routing quirks, or whether one is
configured at all. Any other value is passed through as a platform-specific
system sound name instead (for example a freedesktop sound-theme name such as
`message-new-instant` on Linux, a system sound name such as `Glass` on macOS,
or a toast sound identifier such as `IM` on Windows) and only plays if the
platform recognizes it. On macOS, Notification Center owns playback of system
sound names, while Zetta's built-in tones continue asynchronously after
`zetta notify` exits.

With [shell integration](shell-integration.md) enabled, `zntfy` is an
equivalent shortcut and retains notification command completion.

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
unless `--notify` is present. Run `zetta attention --help` for complete syntax.

## Clipboard

`zetta copy` and `zetta paste` read standard input to the system clipboard and
write the clipboard's text contents to standard output, mirroring macOS's
`pbcopy`/`pbpaste`:

```sh
echo "Build finished" | zetta copy
zetta copy < release-notes.txt
zetta paste > release-notes-copy.txt
zetta paste | grep TODO
```

Both accept `-pboard {general | ruler | find | font}` for `pbcopy`/`pbpaste`
compatibility; Zetta has only one clipboard, so the value is validated but
otherwise has no effect. `zetta paste` also accepts `-Prefer {txt | rtf | ps}`
for the same reason: Zetta only ever stores plain UTF-8 text, so the
preference is validated but does not change the output. `zetta paste` prints
nothing, without an error, if the clipboard is empty or holds no text. Run
`zetta copy --help` or `zetta paste --help` for complete syntax.

On Linux and BSD, the X11 and Wayland clipboards are only available while
their owning process keeps running, so `zetta copy` forks a short-lived
background process that keeps serving the clipboard after the command exits.
macOS and Windows keep the clipboard through their own system services, so no
such process is needed there.

With [shell integration](shell-integration.md) enabled, `zcopy` and `zpaste`
are equivalent shortcuts and retain command completion. On every platform
other than macOS (which already has real `pbcopy`/`pbpaste`), the integration
also defines `pbcopy` and `pbpaste` as the same shortcuts, taking priority
over any preexisting `pbcopy`/`pbpaste` alias (for example one pointing at
`xclip`) so that muscle memory keeps working there too.
