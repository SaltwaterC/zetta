# Installing Zetta

## Build and run

Zetta uses the Rust toolchain pinned in `rust-toolchain.toml`. Initialize the
Zed submodule before the first build:

```sh
git submodule update --init
cargo run
```

Use `cargo check` for the fastest feedback while editing. Release builds use
incremental compilation to reduce rebuild time between local changes and emit
a stripped executable.

## Linux build requirements

Linux defaults to Wayland. Build with `make build X11=1` to include the X11
backend as well. GPUI currently links both xkbcommon libraries on Linux, so
Debian and Ubuntu builds require these packages even for the default Wayland
build:

```sh
sudo apt install libfontconfig-dev libxkbcommon-dev libxkbcommon-x11-dev
```

The `notifications` feature (enabled by default; see
[Serial and network tools](tools.md#desktop-notifications)) plays its built-in
notification tones through ALSA, which requires the ALSA development package
at build time:

```sh
sudo apt install libasound2-dev
```

Building with `NOTIFY=0` (or `--no-default-features` without re-enabling
`notifications`) omits this requirement along with the rest of `zetta notify`.

The `clipboard` feature (enabled by default; see
[Serial and network tools](tools.md#clipboard)) needs no extra system
packages to build. Building with `CLIPBOARD=0` (or `--no-default-features`
without re-enabling `clipboard`) omits `zetta copy`, `zetta paste`, and the
`zcopy`/`zpaste`/`pbcopy`/`pbpaste` shell integration aliases.

## macOS build and runtime requirements

macOS builds require CMake and the full Xcode installation. Install CMake with
Homebrew or otherwise ensure that `cmake` is available on `PATH`:

```sh
brew install cmake
```

Select the full Xcode developer directory rather than the standalone Command
Line Tools directory, then accept the Xcode license if prompted:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license
```

GPUI requires Xcode's Metal compiler. Confirm that the selected developer
directory provides it before building:

```sh
xcrun --find metal
```

The graphical application also requires a Metal-capable GPU at runtime. This
is separate from the Metal compiler requirement above: installing Xcode makes
the compiler available but does not provide a Metal device. Virtual machines
must expose a Metal-compatible GPU to the guest; otherwise Zetta's CLI
commands work, but the graphical application exits during GPUI startup. Check
the reported display and Metal support with:

```sh
system_profiler SPDisplaysDataType
```

## Windows

Build a release executable from PowerShell with Chocolatey's GNU Make:

```powershell
make build
```

The build target locates the Visual Studio C++ toolchain with `vswhere.exe` and
initializes its x64 build environment automatically. The **Desktop development
with C++** workload must be installed.

The build produces the following runtime files in `target\release`:

- `zetta.exe`, the console executable
- `zetta-gui.exe`, the no-console launcher used by the Start Menu shortcut
- `conpty.dll`
- `OpenConsole.exe`

All four files are required. Both executables contain the application icon.

Install Zetta for the current user without administrator privileges:

```powershell
make install
```

This copies the runtime to `%LOCALAPPDATA%\Programs\Zetta`, adds that directory
to the user `PATH`, and creates a Start Menu shortcut. New console sessions can
then run `zetta`. The shortcut launches `zetta-gui.exe`, which starts the
console-native executable without opening an extra console window.

Zetta can be reinstalled while it is running. Windows keeps the previous
runtime under names such as `zetta.old.exe` until its processes exit. Repeating
an identical install preserves that rollback generation; the next changed
install removes it before activating the new one.

The shortcut exposes available profiles in its Windows Jump List, including
when Zetta appears in Start Menu search. Zetta refreshes the entries after
startup and configuration reloads.

Additional installation targets are:

- `make install-binary` updates only the installed executables.
- `make install-assets` recreates only the Start Menu shortcut and requires an
  installed binary.
- `make uninstall` removes the installed runtime, managed `PATH` entry, and
  shortcut.

## Linux desktop integration

Zetta uses `Zetta` as its Wayland application ID and X11 `WM_CLASS`. Build and
install the release binary, desktop entry, and icons under `/usr` with:

```sh
make build
sudo make install
```

To build a restricted binary, pass build flags to both the build and install
steps. `SERIAL`, `HTTP`, `TFTP`, `TFTP_SERVER`, `TFTP_CLIENT`, `NOTIFY`, and
`CLIPBOARD` accept `0`, `false`, `no`, and `off`. `TFTP=0` disables both TFTP
components; the server and client switches can be used independently:

```sh
make build SERIAL=0 HTTP=0 TFTP=0 NOTIFY=0 CLIPBOARD=0
sudo make install SERIAL=0 HTTP=0 TFTP=0 NOTIFY=0 CLIPBOARD=0

# Keep only the TFTP client.
make build TFTP_SERVER=0

# Include X11 support alongside the default Wayland backend.
make build X11=1
```

Disabled tools are omitted from the command palette, default keybindings, and
their implementation is not compiled into the binary. A build without the
TFTP server does not request `cap_net_bind_service` during installation.

When invoked through `sudo`, `make install` uses the existing release artifact
and does not run Cargo again. It grants the binary only the
`cap_net_bind_service` capability needed by the TFTP server to bind UDP port
69. Ubuntu provides `setcap` in `libcap2-bin`.

An unprivileged install builds first but cannot grant that capability. Enable
it separately when required:

```sh
sudo make install-capabilities PREFIX="$HOME/.local"
```

Other supported installation forms are:

- `sudo make install-assets` reinstalls only the desktop entry and icons.
- `sudo make uninstall-assets` removes only those assets.
- `make uninstall` removes the binary and assets.
- `PREFIX=/usr/local` selects a traditional local-system prefix.
- `PREFIX="$HOME/.local"` performs a per-user install without `sudo`.
- `DESTDIR` stages a package build.

Staged installs do not receive filesystem capabilities; packages must apply
`cap_net_bind_service` through their install or post-install metadata. Desktop
and icon caches are refreshed when their utilities are available and `DESTDIR`
is not set.

### WSLg

WSLg exports only applications discovered in system desktop-entry directories,
so use the default `/usr` prefix. Zetta installs 128 px and 512 px hicolor
icons; WSLg requires the 128 px icon for application lookup.

After installing or upgrading under WSL2, close all Zetta windows and run the
following from Windows if the old taskbar icon remains cached:

```powershell
wsl --shutdown
```

## Next steps

See [Using Zetta](usage.md) for the main controls and
[Configuration](configuration.md) for platform-specific configuration paths,
profiles, themes, and key bindings.
