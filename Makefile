CARGO ?= cargo
ENV ?= env
INSTALL ?= install
PREFIX ?= /usr
DESTDIR ?=

export CARGO

APP_ID := Zetta
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
APPLICATIONS_DIR := $(DATADIR)/applications
ICON_128_DIR := $(DATADIR)/icons/hicolor/128x128/apps
ICON_512_DIR := $(DATADIR)/icons/hicolor/512x512/apps

.PHONY: build install install-binary install-assets uninstall uninstall-binary \
	uninstall-assets refresh-desktop-caches

ifeq ($(OS),Windows_NT)
build:
	cmd.exe /d /c scripts\build-windows.cmd
else
build:
	$(ENV) -u DESTDIR $(CARGO) build --release --locked
endif

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
	$(MAKE) install-assets

install-binary:
	$(INSTALL) -Dm755 target/release/zetta $(BINDIR)/zetta

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
