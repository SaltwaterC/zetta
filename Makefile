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
X11 ?= 0

UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
PREFIX ?= /usr/local
else
PREFIX ?= /usr
endif

# Set any of SERIAL, HTTP, TFTP, TFTP_SERVER, TFTP_CLIENT, NOTIFY, or
# CLIPBOARD to 0, false, no, or off to omit that tool from the built binary.
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
ifneq ($(call tool_enabled,$(X11)),)
BUILD_FEATURES += x11
endif

export SERIAL HTTP TFTP TFTP_SERVER TFTP_CLIENT NOTIFY CLIPBOARD X11

export CARGO

APP_ID := Zetta
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
APPLICATIONS_DIR := $(DATADIR)/applications
ICON_128_DIR := $(DATADIR)/icons/hicolor/128x128/apps
ICON_512_DIR := $(DATADIR)/icons/hicolor/512x512/apps
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
MAC_APPLICATIONS_DIR ?= /Applications
MAC_BUNDLE := $(DESTDIR)$(MAC_APPLICATIONS_DIR)/$(APP_ID).app
MAC_RUNTIME_BUNDLE := $(MAC_APPLICATIONS_DIR)/$(APP_ID).app
MAC_CLI_PATH := $(DESTDIR)$(PREFIX)/bin/zetta

.PHONY: build test install install-binary install-capabilities install-assets uninstall \
	uninstall-binary uninstall-assets refresh-desktop-caches

test:
	$(CARGO) test --locked --no-default-features --features "$(BUILD_FEATURES)"

ifeq ($(OS),Windows_NT)
build:
	cmd.exe /d /c scripts\build-windows.cmd

install: build
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action Install

install-binary:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action InstallBinary

install-capabilities:

install-assets:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action InstallShortcut

uninstall:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action Uninstall

uninstall-binary:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action UninstallBinary

uninstall-assets:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Action UninstallShortcut

refresh-desktop-caches:
else ifeq ($(UNAME_S),Darwin)
build:
	$(ENV) -u DESTDIR $(CARGO) build --release --locked --no-default-features --features "$(BUILD_FEATURES)"

install:
	@if [ "$$(id -u)" -eq 0 ]; then \
		test -x target/release/zetta || { \
			echo "target/release/zetta is missing; run 'make build' without sudo first" >&2; \
			exit 1; \
		}; \
	else \
		$(MAKE) build; \
	fi
	$(MAKE) install-binary
	$(MAKE) install-capabilities
	$(MAKE) install-assets

install-binary:
	mkdir -p "$(MAC_BUNDLE)/Contents/MacOS" "$(BINDIR)"
	$(INSTALL) -m 755 target/release/zetta "$(MAC_BUNDLE)/Contents/MacOS/zetta"
	$(RM) "$(MAC_CLI_PATH)"
	sed 's|@MAC_RUNTIME_BUNDLE@|$(MAC_RUNTIME_BUNDLE)|g' resources/macos/zetta-cli.in > "$(MAC_CLI_PATH)"
	chmod 755 "$(MAC_CLI_PATH)"

install-capabilities:

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

uninstall-assets:
	rm -rf "$(MAC_BUNDLE)"

refresh-desktop-caches:
else
build:
	$(ENV) -u DESTDIR $(CARGO) build --release --locked --no-default-features --features "$(BUILD_FEATURES)"

install:
	@if [ "$$(id -u)" -eq 0 ]; then \
		test -x target/release/zetta || { \
			echo "target/release/zetta is missing; run 'make build' without sudo first" >&2; \
			exit 1; \
		}; \
	else \
		$(MAKE) build; \
	fi
	$(MAKE) install-binary
	$(MAKE) install-capabilities
	$(MAKE) install-assets

install-binary:
	$(INSTALL) -Dm755 target/release/zetta $(BINDIR)/zetta

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

install-assets:
	$(INSTALL) -Dm644 resources/linux/$(APP_ID).desktop \
		$(APPLICATIONS_DIR)/$(APP_ID).desktop
	$(INSTALL) -Dm644 assets/icons/zetta-terminal-icon-128.png \
		$(ICON_128_DIR)/$(APP_ID).png
	$(INSTALL) -Dm644 assets/icons/zetta-terminal-icon-512.png \
		$(ICON_512_DIR)/$(APP_ID).png
	$(MAKE) refresh-desktop-caches

uninstall:
	$(MAKE) uninstall-binary
	$(MAKE) uninstall-assets

uninstall-binary:
	$(RM) $(BINDIR)/zetta

uninstall-assets:
	$(RM) $(APPLICATIONS_DIR)/$(APP_ID).desktop
	$(RM) $(ICON_128_DIR)/$(APP_ID).png
	$(RM) $(ICON_512_DIR)/$(APP_ID).png
	$(MAKE) refresh-desktop-caches

refresh-desktop-caches:
	@if [ -z "$(DESTDIR)" ]; then \
		if command -v update-desktop-database >/dev/null 2>&1; then \
			update-desktop-database "$(PREFIX)/share/applications"; \
		fi; \
		if command -v gtk-update-icon-cache >/dev/null 2>&1 \
			&& [ -f "$(PREFIX)/share/icons/hicolor/index.theme" ]; then \
			gtk-update-icon-cache -f "$(PREFIX)/share/icons/hicolor"; \
		fi; \
	fi
endif
