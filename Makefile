CARGO ?= cargo
ENV ?= env
INSTALL ?= install
SETCAP ?= setcap
PREFIX ?= /usr
DESTDIR ?=

export CARGO

APP_ID := Zetta
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
APPLICATIONS_DIR := $(DATADIR)/applications
ICON_128_DIR := $(DATADIR)/icons/hicolor/128x128/apps
ICON_512_DIR := $(DATADIR)/icons/hicolor/512x512/apps

.PHONY: build install install-binary install-capabilities install-assets uninstall \
	uninstall-binary uninstall-assets refresh-desktop-caches

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
else
build:
	$(ENV) -u DESTDIR $(CARGO) build --release --locked

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
	@if [ "$$(uname -s)" = "Linux" ]; then \
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
