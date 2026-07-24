# Serial and network tools

Zetta includes serial-console support, static HTTP and TFTP servers, and a
command-line TFTP client. The servers have no authentication or encryption;
expose them only on networks whose clients you trust.

## Serial console

Press `Ctrl-Shift-S` or choose **Zetta: Toggle Serial Console** from the command
palette to enumerate serial devices and connect one in a new left/right split.

The dialog defaults to 115200 baud, 8 data bits, no parity, 1 stop bit, and no
flow control (115200 8N1). Use `Tab` to move between settings, arrow keys to
change the selected value, and `Ctrl-R`/`Cmd-R` to rescan. Baud rate, data bits,
parity, stop bits, and software or hardware flow control are configurable.

On Linux, placeholder legacy UART nodes are validated before display. When no
usable ports are detected, the dialog reports that no devices were found.
Closing the pane closes the device.

## TFTP server

Choose **Zetta: Start TFTP Server** from the command palette to serve files
below Zetta's launch directory on UDP port 69. Zetta opens the server log in a
new left/right pane, and each entry includes a human-readable UTC timestamp.
Press `Ctrl-C` in that pane, or close it, to stop the server.

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
directory over TCP port 8000. For example:

```sh
wget http://HOST:8000/firmware.bin
```

The server supports read-only `GET` and `HEAD`, serves `index.html` when
present, generates a browsable index for other directories, and logs each
request in a new pane. Absolute paths, parent traversal, and symlinks resolving
outside the served directory are rejected. Press `Ctrl-C` in the server pane,
or close it, to stop the server.

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
