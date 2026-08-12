CARGO ?= cargo
ENV ?= env
INSTALL ?= install
SETCAP ?= setcap
DESTDIR ?=
SERIAL ?= 1
HTTP ?= 1
TFTP ?= 1
TFTP_SERVER ?= $(TFTP)
TFTP_CLIENT ?= $(TFTP)
NOTIFY ?= 1
CLIPBOARD ?= 1
SYNTAX_HIGHLIGHTING ?= 1
X11 ?= 0
RELEASE ?= 0

UNAME_S := $(shell uname -s)
ifneq ($(OS),Windows_NT)
IS_ROOT := $(shell test "$$(id -u)" -eq 0 && echo 1)
else
IS_ROOT :=
endif
ifeq ($(UNAME_S),Darwin)
ifeq ($(IS_ROOT),1)
PREFIX ?= /usr/local
MAC_APPLICATIONS_DIR ?= /Applications
else
PREFIX ?= $(HOME)/.local
MAC_APPLICATIONS_DIR ?= $(HOME)/Applications
endif
else ifeq ($(UNAME_S),Linux)
ifeq ($(IS_ROOT),1)
PREFIX ?= /usr
else
PREFIX ?= $(HOME)/.local/zetta.app
endif
else
PREFIX ?= /usr
endif

ifneq ($(filter 1 true yes on,$(strip $(RELEASE))),)
BUILD_PROFILE := release
CARGO_PROFILE_ARGS := --release
else
BUILD_PROFILE := debug
CARGO_PROFILE_ARGS :=
endif
BUILD_TARGET_DIR := target/$(BUILD_PROFILE)

ifeq ($(OS),Windows_NT)
CARGO_BUILD_JOBS ?= $(shell powershell.exe -NoProfile -Command "[Environment]::ProcessorCount")
else ifeq ($(UNAME_S),Darwin)
CARGO_BUILD_JOBS ?= $(shell sysctl -n hw.ncpu)
else ifeq ($(UNAME_S),Linux)
CARGO_BUILD_JOBS ?= $(shell nproc)
else
CARGO_BUILD_JOBS ?= $(shell getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
endif
CARGO_BUILD_JOBS := $(strip $(CARGO_BUILD_JOBS))
ifeq ($(CARGO_BUILD_JOBS),)
CARGO_BUILD_JOBS := 1
endif

# Set any of SERIAL, HTTP, TFTP, TFTP_SERVER, TFTP_CLIENT, NOTIFY, CLIPBOARD,
# or SYNTAX_HIGHLIGHTING to 0, false, no, or off to omit that capability from
# the built binary.
# TFTP is a convenient shorthand for disabling both the server and client.
# Set X11=1 to include the X11 backend alongside the default Wayland backend.
tool_enabled = $(if $(filter 0 false no off,$(strip $(1))),,1)
BUILD_FEATURES := wayland
ifneq ($(call tool_enabled,$(SERIAL)),)
BUILD_FEATURES += serial-console
endif
ifneq ($(call tool_enabled,$(HTTP)),)
BUILD_FEATURES += http-server
endif
ifneq ($(call tool_enabled,$(TFTP_SERVER)),)
BUILD_FEATURES += tftp-server
endif
ifneq ($(call tool_enabled,$(TFTP_CLIENT)),)
BUILD_FEATURES += tftp-client
endif
ifneq ($(call tool_enabled,$(NOTIFY)),)
BUILD_FEATURES += notifications
endif
ifneq ($(call tool_enabled,$(CLIPBOARD)),)
BUILD_FEATURES += clipboard
endif
ifneq ($(call tool_enabled,$(SYNTAX_HIGHLIGHTING)),)
BUILD_FEATURES += syntax-highlighting
endif
ifneq ($(call tool_enabled,$(X11)),)
BUILD_FEATURES += x11
endif

export SERIAL HTTP TFTP TFTP_SERVER TFTP_CLIENT NOTIFY CLIPBOARD SYNTAX_HIGHLIGHTING X11
export CARGO_BUILD_JOBS

export CARGO

APP_ID := Zetta
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
APPLICATIONS_DIR := $(DATADIR)/applications
ICON_128_DIR := $(DATADIR)/icons/hicolor/128x128/apps
ICON_512_DIR := $(DATADIR)/icons/hicolor/512x512/apps
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
MAC_BUNDLE := $(DESTDIR)$(MAC_APPLICATIONS_DIR)/$(APP_ID).app
MAC_RUNTIME_BUNDLE := $(MAC_APPLICATIONS_DIR)/$(APP_ID).app
MAC_CLI_DIR := $(DESTDIR)$(PREFIX)/bin
MAC_CLI_PATH := $(MAC_CLI_DIR)/zetta
LINUX_USER_INSTALL := $(if $(and $(filter Linux,$(UNAME_S)),$(IS_ROOT)),,1)
LINUX_USER_DATA_DIR := $(DESTDIR)$(HOME)/.local/share
LINUX_USER_BIN_DIR := $(DESTDIR)$(HOME)/.local/bin
LINUX_USER_DESKTOP_DIR := $(LINUX_USER_DATA_DIR)/applications
LINUX_USER_CLI_PATH := $(LINUX_USER_BIN_DIR)/zetta

.PHONY: build fmt test lint install install-binary install-capabilities install-assets install-user-path uninstall \
	uninstall-binary uninstall-assets uninstall-user-path refresh-desktop-caches clean

test:
	$(CARGO) test --locked --quiet --no-default-features --features "$(BUILD_FEATURES)" -- --format=terse

fmt:
	$(CARGO) fmt --check

lint:
	$(CARGO) clippy --locked --all-targets --no-default-features --features "$(BUILD_FEATURES)" -- -D warnings

ifeq ($(OS),Windows_NT)
build:
	cmd.exe /d /c scripts\build-windows.cmd $(CARGO_PROFILE_ARGS)

install: build
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action Install -SourceBinary "$(BUILD_TARGET_DIR)/zetta.exe" -SourceGuiBinary "$(BUILD_TARGET_DIR)/zetta-gui.exe"

install-binary:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action InstallBinary -SourceBinary "$(BUILD_TARGET_DIR)/zetta.exe" -SourceGuiBinary "$(BUILD_TARGET_DIR)/zetta-gui.exe"

install-capabilities:

install-assets:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action InstallShortcut

install-user-path:

uninstall-user-path:

uninstall:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action Uninstall

uninstall-binary:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action UninstallBinary

uninstall-assets:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action UninstallShortcut

refresh-desktop-caches:
else ifeq ($(UNAME_S),Darwin)
build:
	$(ENV) -u DESTDIR $(CARGO) build $(CARGO_PROFILE_ARGS) --locked --no-default-features --features "$(BUILD_FEATURES)"

install:
	@if [ "$$(id -u)" -eq 0 ]; then \
		test -x "$(BUILD_TARGET_DIR)/zetta" || { \
			echo "$(BUILD_TARGET_DIR)/zetta is missing; run 'make build$(if $(filter release,$(BUILD_PROFILE)), RELEASE=1,)' without sudo first" >&2; \
			exit 1; \
		}; \
	else \
		$(MAKE) build; \
	fi
	$(MAKE) install-binary
	$(MAKE) install-capabilities
	$(MAKE) install-assets
	$(MAKE) install-user-path

install-binary:
	mkdir -p "$(MAC_BUNDLE)/Contents/MacOS" "$(BINDIR)"
	$(INSTALL) -m 755 "$(BUILD_TARGET_DIR)/zetta" "$(MAC_BUNDLE)/Contents/MacOS/zetta"
	$(RM) "$(MAC_CLI_PATH)"
	sed 's|@MAC_RUNTIME_BUNDLE@|$(MAC_RUNTIME_BUNDLE)|g' resources/macos/zetta-cli.in > "$(MAC_CLI_PATH)"
	chmod 755 "$(MAC_CLI_PATH)"

install-capabilities:

install-user-path:
ifeq ($(IS_ROOT),)
ifeq ($(DESTDIR),)
	sh scripts/install-user-path.sh "$(MAC_CLI_DIR)"
endif
endif

uninstall-user-path:
ifeq ($(IS_ROOT),)
ifeq ($(DESTDIR),)
	sh scripts/install-user-path.sh "$(MAC_CLI_DIR)" uninstall
endif
endif

install-assets:
	@test -x "$(MAC_BUNDLE)/Contents/MacOS/zetta" || { \
		echo "$(MAC_BUNDLE)/Contents/MacOS/zetta is missing; run 'make install-binary' first" >&2; \
		exit 1; \
	}
	scripts/bundle-macos.sh "$(MAC_BUNDLE)" "$(VERSION)"

uninstall:
	$(MAKE) uninstall-binary
	$(MAKE) uninstall-assets

uninstall-binary:
	$(RM) "$(MAC_CLI_PATH)"
	$(RM) "$(MAC_BUNDLE)/Contents/MacOS/zetta"
	$(MAKE) uninstall-user-path

uninstall-assets:
	rm -rf "$(MAC_BUNDLE)"

refresh-desktop-caches:
else
build:
	$(ENV) -u DESTDIR $(CARGO) build $(CARGO_PROFILE_ARGS) --locked --no-default-features --features "$(BUILD_FEATURES)"

install:
	@if [ "$$(id -u)" -eq 0 ]; then \
		test -x "$(BUILD_TARGET_DIR)/zetta" || { \
			echo "$(BUILD_TARGET_DIR)/zetta is missing; run 'make build$(if $(filter release,$(BUILD_PROFILE)), RELEASE=1,)' without sudo first" >&2; \
			exit 1; \
		}; \
	else \
		$(MAKE) build; \
	fi
	$(MAKE) install-binary
	$(MAKE) install-capabilities
	$(MAKE) install-assets
	$(MAKE) install-user-path

install-binary:
	$(INSTALL) -Dm755 "$(BUILD_TARGET_DIR)/zetta" $(BINDIR)/zetta
ifneq ($(LINUX_USER_INSTALL),)
	mkdir -p "$(LINUX_USER_BIN_DIR)"
	$(RM) "$(LINUX_USER_CLI_PATH)"
	ln -s "$(BINDIR)/zetta" "$(LINUX_USER_CLI_PATH)"
endif

install-capabilities:
	@if [ "$$(uname -s)" = "Linux" ] && [ -n "$(call tool_enabled,$(TFTP_SERVER))" ]; then \
		if [ -n "$(DESTDIR)" ]; then \
			echo "Skipping cap_net_bind_service for staged install; apply it in the package install step"; \
		elif [ "$$(id -u)" -ne 0 ]; then \
			echo "Skipping cap_net_bind_service: rerun with sufficient privileges to enable the TFTP server" >&2; \
		else \
			test -x "$(BINDIR)/zetta" || { \
				echo "$(BINDIR)/zetta is missing; run 'make install-binary' first" >&2; \
				exit 1; \
			}; \
			command -v "$(SETCAP)" >/dev/null 2>&1 || { \
				echo "$(SETCAP) is required to grant cap_net_bind_service (install libcap2-bin on Ubuntu)" >&2; \
				exit 1; \
			}; \
			$(SETCAP) cap_net_bind_service=+ep "$(BINDIR)/zetta" || { \
				echo "Could not grant cap_net_bind_service to $(BINDIR)/zetta" >&2; \
				exit 1; \
			}; \
		fi; \
	fi

install-user-path:
ifneq ($(LINUX_USER_INSTALL),)
ifeq ($(DESTDIR),)
	sh scripts/install-user-path.sh "$(LINUX_USER_BIN_DIR)"
endif
endif

uninstall-user-path:
ifneq ($(LINUX_USER_INSTALL),)
ifeq ($(DESTDIR),)
	sh scripts/install-user-path.sh "$(LINUX_USER_BIN_DIR)" uninstall
endif
endif

install-assets:
	$(INSTALL) -Dm644 resources/linux/$(APP_ID).desktop \
		$(APPLICATIONS_DIR)/$(APP_ID).desktop
	$(INSTALL) -Dm644 assets/icons/zetta-terminal-icon-128.png \
		$(ICON_128_DIR)/$(APP_ID).png
	$(INSTALL) -Dm644 assets/icons/zetta-terminal-icon-512.png \
		$(ICON_512_DIR)/$(APP_ID).png
ifneq ($(LINUX_USER_INSTALL),)
	mkdir -p "$(LINUX_USER_DESKTOP_DIR)"
	sed \
		-e 's|^TryExec=.*|TryExec=$(BINDIR)/zetta|' \
		-e 's|^Exec=.*|Exec=$(BINDIR)/zetta|' \
		-e 's|^Icon=.*|Icon=$(ICON_512_DIR)/$(APP_ID).png|' \
		resources/linux/$(APP_ID).desktop > "$(LINUX_USER_DESKTOP_DIR)/$(APP_ID).desktop"
	chmod 644 "$(LINUX_USER_DESKTOP_DIR)/$(APP_ID).desktop"
endif
	$(MAKE) refresh-desktop-caches

uninstall:
	$(MAKE) uninstall-binary
	$(MAKE) uninstall-assets

uninstall-binary:
	$(RM) $(BINDIR)/zetta
ifneq ($(LINUX_USER_INSTALL),)
	$(RM) "$(LINUX_USER_CLI_PATH)"
endif
	$(MAKE) uninstall-user-path

uninstall-assets:
	$(RM) $(APPLICATIONS_DIR)/$(APP_ID).desktop
	$(RM) $(ICON_128_DIR)/$(APP_ID).png
	$(RM) $(ICON_512_DIR)/$(APP_ID).png
ifneq ($(LINUX_USER_INSTALL),)
	$(RM) "$(LINUX_USER_DESKTOP_DIR)/$(APP_ID).desktop"
endif
	$(MAKE) refresh-desktop-caches

refresh-desktop-caches:
	@if [ -z "$(DESTDIR)" ]; then \
		if command -v update-desktop-database >/dev/null 2>&1; then \
			update-desktop-database "$(if $(LINUX_USER_INSTALL),$(LINUX_USER_DESKTOP_DIR),$(PREFIX)/share/applications)"; \
		fi; \
		if command -v gtk-update-icon-cache >/dev/null 2>&1 \
			&& [ -f "$(PREFIX)/share/icons/hicolor/index.theme" ]; then \
			gtk-update-icon-cache -f "$(PREFIX)/share/icons/hicolor"; \
		fi; \
	fi

clean:
	$(CARGO) clean

endif
