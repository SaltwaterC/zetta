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
SESSION_PERSISTENCE ?= 1
WORKTREE ?= 1
X11 ?= 0
RELEASE ?= 0

ifneq ($(OS),Windows_NT)
UNAME_S := $(shell uname -s)
IS_ROOT := $(shell test "$$(id -u)" -eq 0 && echo 1)
else
UNAME_S :=
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

# These packages are intentionally standalone Cargo workspaces rather than
# members of Zetta's root package. Keep them in the top-level checks so a
# change under crates/ cannot silently skip formatting or tests.
ZETTA_CRATE_DIRS := \
	crates/alacritty_terminal \
	crates/gpui_linux \
	crates/gpui_macos \
	crates/gpui_platform \
	crates/gpui_windows \
	crates/terminal \
	crates/terminal_view \
	crates/zwt \
	crates/zmux
# These standalone test workspaces have checked-in lockfiles. The other local
# platform crates are included in formatting coverage but do not have an
# independent locked test graph.
ZETTA_TEST_CRATE_DIRS := \
	crates/alacritty_terminal \
	crates/terminal \
	crates/zwt \
	crates/zmux

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
# SYNTAX_HIGHLIGHTING, SESSION_PERSISTENCE, or WORKTREE to 0, false, no, or off
# to omit that capability from the built binary.
# TFTP is a convenient shorthand for disabling both the server and client.
# Linux and FreeBSD default to Wayland; set X11=1 to include the X11 backend.
tool_enabled = $(if $(filter 0 false no off,$(strip $(1))),,1)

# The capability features are the same whatever the target is; only the
# windowing backend differs. Keeping them in their own list is what lets the
# cross-check targets below build a feature set for a platform that is not the
# host, without restating which capabilities are on.
CAPABILITY_FEATURES :=
ifneq ($(call tool_enabled,$(SERIAL)),)
CAPABILITY_FEATURES += serial-console
endif
ifneq ($(call tool_enabled,$(HTTP)),)
CAPABILITY_FEATURES += http-server
endif
ifneq ($(call tool_enabled,$(TFTP_SERVER)),)
CAPABILITY_FEATURES += tftp-server
endif
ifneq ($(call tool_enabled,$(TFTP_CLIENT)),)
CAPABILITY_FEATURES += tftp-client
endif
ifneq ($(call tool_enabled,$(NOTIFY)),)
CAPABILITY_FEATURES += notifications
endif
ifneq ($(call tool_enabled,$(CLIPBOARD)),)
CAPABILITY_FEATURES += clipboard
endif
ifneq ($(call tool_enabled,$(SYNTAX_HIGHLIGHTING)),)
CAPABILITY_FEATURES += syntax-highlighting
endif
ifneq ($(call tool_enabled,$(SESSION_PERSISTENCE)),)
CAPABILITY_FEATURES += session-persistence
endif
ifneq ($(call tool_enabled,$(WORKTREE)),)
CAPABILITY_FEATURES += worktree
endif

ifeq ($(OS),Windows_NT)
PLATFORM_FEATURES := windows-gui
else ifeq ($(UNAME_S),Darwin)
PLATFORM_FEATURES :=
else ifneq ($(filter Linux FreeBSD,$(UNAME_S)),)
PLATFORM_FEATURES := wayland
else
PLATFORM_FEATURES :=
endif
ifneq ($(call tool_enabled,$(X11)),)
ifneq ($(filter Linux FreeBSD,$(UNAME_S)),)
PLATFORM_FEATURES += x11
endif
endif

BUILD_FEATURES := $(PLATFORM_FEATURES) $(CAPABILITY_FEATURES)

export SERIAL HTTP TFTP TFTP_SERVER TFTP_CLIENT NOTIFY CLIPBOARD SYNTAX_HIGHLIGHTING SESSION_PERSISTENCE WORKTREE X11
export CARGO_BUILD_JOBS

export CARGO

ifeq ($(OS),Windows_NT)
# Native build scripts fingerprint the Visual Studio environment. Route every
# Cargo command through one initializer so lint, test, and build share cache
# entries instead of alternately invalidating them.
CARGO_RUN := "$(CURDIR)/scripts/cargo-windows.cmd"
else
CARGO_RUN := $(CARGO)
endif

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
MAC_ZWT_CLI_PATH := $(MAC_CLI_DIR)/zwt
LINUX_USER_INSTALL := $(if $(and $(filter Linux,$(UNAME_S)),$(IS_ROOT)),,1)
LINUX_USER_DATA_DIR := $(DESTDIR)$(HOME)/.local/share
LINUX_USER_BIN_DIR := $(DESTDIR)$(HOME)/.local/bin
LINUX_USER_DESKTOP_DIR := $(LINUX_USER_DATA_DIR)/applications
LINUX_USER_CLI_PATH := $(LINUX_USER_BIN_DIR)/zetta
LINUX_USER_ZWT_PATH := $(LINUX_USER_BIN_DIR)/zwt

WINDOWS_ZWT_ARGS := $(if $(call tool_enabled,$(WORKTREE)), -SourceZwtBinary "$(BUILD_TARGET_DIR)/zwt.exe",)

.PHONY: all build fmt test lint check-platforms check-features \
	check-linux check-windows check-macos \
	can-check-linux can-check-windows can-check-macos \
	install install-binary install-capabilities install-assets install-user-path uninstall \
	uninstall-binary uninstall-assets uninstall-user-path refresh-desktop-caches clean

all: fmt lint test build

# Parallel execution helper: runs command for each item in $(1), collects results
# Usage: $(call parallel_for,items,command_template,description)
# command_template should use $$item for the current item
define parallel_for
	@items="$(1)"; \
	cmd_template='$(2)'; \
	desc="$(3)"; \
	tmpdir=$$(mktemp -d); \
	trap 'rm -rf "$$tmpdir"' EXIT; \
	pids=""; \
	idx=0; \
	for item in $$items; do \
		idx=$$((idx + 1)); \
		( \
			eval "$$cmd_template" >"$$tmpdir/out.$$idx" 2>"$$tmpdir/err.$$idx"; \
			echo $$? >"$$tmpdir/status.$$idx"; \
			echo "$$item" >"$$tmpdir/name.$$idx"; \
		) & \
		pids="$$pids $$!"; \
	done; \
	failed=0; \
	for pid in $$pids; do \
		wait $$pid || true; \
	done; \
	for f in "$$tmpdir"/name.*; do \
		[ -f "$$f" ] || continue; \
		idx=$${f##*.}; \
		name=$$(cat "$$f"); \
		status=$$(cat "$$tmpdir/status.$$idx"); \
		if [ "$$status" -eq 0 ]; then \
			printf "  \033[32m✓\033[0m %s\n" "$$name"; \
		else \
			printf "  \033[31m✗\033[0m %s\n" "$$name"; \
			failed=1; \
		fi; \
	done; \
	if [ $$failed -eq 1 ]; then \
		echo ""; \
		echo "$(3) failures:"; \
		for f in "$$tmpdir"/name.*; do \
			[ -f "$$f" ] || continue; \
			idx=$${f##*.}; \
			name=$$(cat "$$f"); \
			status=$$(cat "$$tmpdir/status.$$idx"); \
			if [ "$$status" -ne 0 ]; then \
				echo "--- $$name ---"; \
				cat "$$tmpdir/out.$$idx" 2>/dev/null || true; \
				cat "$$tmpdir/err.$$idx" 2>/dev/null || true; \
				echo ""; \
			fi; \
		done; \
		exit 1; \
	fi
endef

test:
	$(CARGO_RUN) test --locked --quiet --no-default-features --features "$(BUILD_FEATURES)" -- --format=terse
	$(call parallel_for,$(ZETTA_TEST_CRATE_DIRS), \
		cd "$$item" && \
		if [ "$$item" = "crates/zmux" ]; then \
			$(CARGO_RUN) build --locked --bin zmux --bin zmux-pty; \
		fi && \
		$(CARGO_RUN) test --locked --quiet -- --format=terse, \
		Crate tests)

fmt:
	$(CARGO) fmt --check
	$(call parallel_for,$(ZETTA_CRATE_DIRS), \
		$(CARGO) fmt --manifest-path "$$item/Cargo.toml" --check, \
		Format check)

lint:
	$(CARGO_RUN) clippy --locked --all-targets --no-default-features --features "$(BUILD_FEATURES)" -- -D warnings

# Per-platform checking.
#
# `make test` compiles only the host's `cfg` arms, so a change to
# `#[cfg(windows)]` or `#[cfg(target_os = "macos")]` code can pass every check
# on one machine and still fail to build on another. Each target below checks
# one platform: natively when that platform *is* the host, and through its
# cross target otherwise. Zetta is developed from all three, so which of these
# is the cheap one depends on where you are sitting.
#
# All of them pass `--all-targets`, so the tests behind those `cfg`s are
# compiled too — a plain `cargo check` skips them, which is how test code that
# does not build under a feature combination goes unnoticed.
#
# They check; they do not link or run. Running a platform's tests still needs a
# machine of that platform, so a green `check-windows` is not a green Windows
# test suite.
ifeq ($(OS),Windows_NT)
HOST_PLATFORM := windows
else ifeq ($(UNAME_S),Darwin)
HOST_PLATFORM := macos
else ifeq ($(UNAME_S),Linux)
HOST_PLATFORM := linux
else
HOST_PLATFORM := other
endif

CHECK_LINUX_TARGET := x86_64-unknown-linux-gnu
CHECK_WINDOWS_TARGET := x86_64-pc-windows-gnu
CHECK_MACOS_TARGET := x86_64-apple-darwin
CHECK_LINUX_FEATURES := wayland $(CAPABILITY_FEATURES)
CHECK_WINDOWS_FEATURES := windows-gui $(CAPABILITY_FEATURES)
CHECK_MACOS_FEATURES := $(CAPABILITY_FEATURES)

# A cross check needs more than the Rust target: aws-lc-sys, ring, tree-sitter
# and wasmtime all compile C or assembly against the target's own headers, so
# there has to be a C toolchain that can produce them. That is where a missing
# prerequisite otherwise surfaces — several screens into a build script — so
# each platform is probed for one first.
#
# `cc-rs` reads CC_<target with underscores>, which is also how osxcross and the
# Homebrew cross-gcc packages are usually wired up, so an explicit setting
# counts as well as a well-known binary on PATH.
linux_cc_present = { [ -n "$$CC_x86_64_unknown_linux_gnu" ] || command -v x86_64-linux-gnu-gcc >/dev/null 2>&1; }
windows_cc_present = { [ -n "$$CC_x86_64_pc_windows_gnu" ] || command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; }
macos_cc_present = { [ -n "$$CC_x86_64_apple_darwin" ] || command -v o64-clang >/dev/null 2>&1; }
rust_target_installed = rustup target list --installed 2>/dev/null | grep -qx "$(1)"

# Whether a platform is checkable at all, as an exit status and nothing else.
# Kept apart from the check so `check-platforms` can tell "no toolchain here" from
# "this platform does not compile", and report the second as the failure it is.
can-check-linux:
	@[ "$(HOST_PLATFORM)" = "linux" ] || { \
		$(call rust_target_installed,$(CHECK_LINUX_TARGET)) && $(linux_cc_present); }

can-check-windows:
	@[ "$(HOST_PLATFORM)" = "windows" ] || { \
		$(call rust_target_installed,$(CHECK_WINDOWS_TARGET)) && $(windows_cc_present); }

can-check-macos:
	@[ "$(HOST_PLATFORM)" = "macos" ] || { \
		$(call rust_target_installed,$(CHECK_MACOS_TARGET)) && $(macos_cc_present); }

check-linux:
	@if [ "$(HOST_PLATFORM)" = "linux" ]; then exit 0; fi; \
	$(call rust_target_installed,$(CHECK_LINUX_TARGET)) || { \
		echo "$@: the $(CHECK_LINUX_TARGET) Rust target is not installed."; \
		echo "  rustup target add $(CHECK_LINUX_TARGET)"; \
		exit 1; }; \
	$(linux_cc_present) || { \
		echo "$@: no Linux cross toolchain (x86_64-linux-gnu-gcc)."; \
		echo "  Dependencies compile C for the target, so the Rust target alone"; \
		echo "  is not enough. On macOS: brew install"; \
		echo "  messense/macos-cross-toolchains/x86_64-unknown-linux-gnu."; \
		echo "  Otherwise point CC_x86_64_unknown_linux_gnu at a cross gcc."; \
		exit 1; }
	$(CARGO_RUN) check --locked --all-targets \
		$(if $(filter linux,$(HOST_PLATFORM)),,--target $(CHECK_LINUX_TARGET)) \
		--no-default-features --features "$(CHECK_LINUX_FEATURES)"

check-windows:
	@if [ "$(HOST_PLATFORM)" = "windows" ]; then exit 0; fi; \
	$(call rust_target_installed,$(CHECK_WINDOWS_TARGET)) || { \
		echo "$@: the $(CHECK_WINDOWS_TARGET) Rust target is not installed."; \
		echo "  rustup target add $(CHECK_WINDOWS_TARGET)"; \
		exit 1; }; \
	$(windows_cc_present) || { \
		echo "$@: no MinGW-w64 C toolchain (x86_64-w64-mingw32-gcc)."; \
		echo "  Dependencies compile C for the target, so the Rust target alone"; \
		echo "  is not enough. Install your platform's mingw-w64 package"; \
		echo "  (apt/dnf mingw-w64, or brew install mingw-w64)."; \
		exit 1; }
	$(CARGO_RUN) check --locked --all-targets \
		$(if $(filter windows,$(HOST_PLATFORM)),,--target $(CHECK_WINDOWS_TARGET)) \
		--no-default-features --features "$(CHECK_WINDOWS_FEATURES)"

check-macos:
	@if [ "$(HOST_PLATFORM)" = "macos" ]; then exit 0; fi; \
	$(call rust_target_installed,$(CHECK_MACOS_TARGET)) || { \
		echo "$@: the $(CHECK_MACOS_TARGET) Rust target is not installed."; \
		echo "  rustup target add $(CHECK_MACOS_TARGET)"; \
		exit 1; }; \
	$(macos_cc_present) || { \
		echo "$@: no macOS cross toolchain."; \
		echo "  The Rust target alone is not enough: dependencies compile C against"; \
		echo "  the Apple SDK, which a Linux cc and a bare clang do not have — cc"; \
		echo "  rejects -arch, and clang falls back to /usr/include and fails on"; \
		echo "  glibc headers. Install osxcross with an Apple SDK, then put"; \
		echo "  o64-clang on PATH or set CC_x86_64_apple_darwin to it."; \
		exit 1; }
	$(CARGO_RUN) check --locked --all-targets \
		$(if $(filter macos,$(HOST_PLATFORM)),,--target $(CHECK_MACOS_TARGET)) \
		--no-default-features --features "$(CHECK_MACOS_FEATURES)"

# The two feature combinations AGENTS.md calls out: `x11` covers Linux
# windowing-backend selection, and either one with no capability features
# exercises every `cli_services`/`servers_enabled`/`tftp_enabled` gate.
check-features:
	$(CARGO_RUN) check --locked --all-targets --no-default-features --features x11
	$(CARGO_RUN) check --locked --all-targets --no-default-features --features wayland

# Every platform this machine can check. The host is always one of them; a
# platform whose cross toolchain is missing is skipped rather than failing the
# run, so this stays useful wherever it is invoked from. A platform that *can*
# be checked and does not compile fails, which is the whole point. Run
# `make check-linux`, `check-windows` or `check-macos` directly to be told what
# a skip is missing.
check-platforms: check-features
	@failed=0; \
	for platform in linux windows macos; do \
		if $(MAKE) --no-print-directory can-check-$$platform >/dev/null 2>&1; then \
			if $(MAKE) --no-print-directory check-$$platform; then \
				printf "  \033[32m✓\033[0m %s%s\n" "$$platform" \
					"$$([ "$$platform" = "$(HOST_PLATFORM)" ] && echo ' (host)')"; \
			else \
				printf "  \033[31m✗\033[0m %s\n" "$$platform"; \
				failed=1; \
			fi; \
		else \
			printf "  \033[33m—\033[0m %s (no toolchain; run 'make check-%s')\n" \
				"$$platform" "$$platform"; \
		fi; \
	done; \
	exit $$failed

ifeq ($(OS),Windows_NT)
build:
	cmd.exe /d /c scripts\build-windows.cmd $(CARGO_PROFILE_ARGS)

install: build
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action Install -SourceBinary "$(BUILD_TARGET_DIR)/zetta.exe" -SourceGuiBinary "$(BUILD_TARGET_DIR)/zetta-gui.exe" -SourceMuxBinary "$(BUILD_TARGET_DIR)/zmux.exe" -SourcePtyBinary "$(BUILD_TARGET_DIR)/zmux-pty.exe"$(WINDOWS_ZWT_ARGS)

install-binary:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-windows.ps1 -Action InstallBinary -SourceBinary "$(BUILD_TARGET_DIR)/zetta.exe" -SourceGuiBinary "$(BUILD_TARGET_DIR)/zetta-gui.exe" -SourceMuxBinary "$(BUILD_TARGET_DIR)/zmux.exe" -SourcePtyBinary "$(BUILD_TARGET_DIR)/zmux-pty.exe"$(WINDOWS_ZWT_ARGS)

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
	$(ENV) -u DESTDIR $(CARGO_RUN) build $(CARGO_PROFILE_ARGS) --locked --no-default-features --features "$(BUILD_FEATURES)"

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
	$(INSTALL) -m 755 "$(BUILD_TARGET_DIR)/zmux" "$(MAC_BUNDLE)/Contents/MacOS/zmux"
	if [ -n "$(call tool_enabled,$(WORKTREE))" ]; then \
		$(INSTALL) -m 755 "$(BUILD_TARGET_DIR)/zwt" "$(MAC_BUNDLE)/Contents/MacOS/zwt"; \
	else \
		$(RM) "$(MAC_BUNDLE)/Contents/MacOS/zwt"; \
	fi
	$(RM) "$(MAC_CLI_PATH)"
	sed 's|@MAC_RUNTIME_BUNDLE@|$(MAC_RUNTIME_BUNDLE)|g' resources/macos/zetta-cli.in > "$(MAC_CLI_PATH)"
	chmod 755 "$(MAC_CLI_PATH)"
	if [ -n "$(call tool_enabled,$(WORKTREE))" ]; then \
		$(RM) "$(MAC_ZWT_CLI_PATH)"; \
		sed 's|@MAC_RUNTIME_BUNDLE@|$(MAC_RUNTIME_BUNDLE)|g' resources/macos/zwt-cli.in > "$(MAC_ZWT_CLI_PATH)"; \
		chmod 755 "$(MAC_ZWT_CLI_PATH)"; \
	else \
		$(RM) "$(MAC_ZWT_CLI_PATH)"; \
	fi

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
	$(RM) "$(MAC_ZWT_CLI_PATH)"
	$(RM) "$(MAC_BUNDLE)/Contents/MacOS/zetta"
	$(RM) "$(MAC_BUNDLE)/Contents/MacOS/zmux"
	$(RM) "$(MAC_BUNDLE)/Contents/MacOS/zwt"
	$(MAKE) uninstall-user-path

uninstall-assets:
	rm -rf "$(MAC_BUNDLE)"

refresh-desktop-caches:
else
build:
	$(ENV) -u DESTDIR $(CARGO_RUN) build $(CARGO_PROFILE_ARGS) --locked --no-default-features --features "$(BUILD_FEATURES)"

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
	# The multiplexer holds background sessions, so it has to be installed
	# beside Zetta: a client starts it from its own directory rather than
	# through PATH, where an unrelated zmux could be picked up instead.
	$(INSTALL) -Dm755 "$(BUILD_TARGET_DIR)/zmux" $(BINDIR)/zmux
ifneq ($(call tool_enabled,$(WORKTREE)),)
	$(INSTALL) -Dm755 "$(BUILD_TARGET_DIR)/zwt" $(BINDIR)/zwt
else
	$(RM) $(BINDIR)/zwt
endif
ifneq ($(LINUX_USER_INSTALL),)
	mkdir -p "$(LINUX_USER_BIN_DIR)"
	$(RM) "$(LINUX_USER_CLI_PATH)"
	ln -s "$(BINDIR)/zetta" "$(LINUX_USER_CLI_PATH)"
	$(RM) "$(LINUX_USER_BIN_DIR)/zmux"
	ln -s "$(BINDIR)/zmux" "$(LINUX_USER_BIN_DIR)/zmux"
ifneq ($(call tool_enabled,$(WORKTREE)),)
	$(RM) "$(LINUX_USER_ZWT_PATH)"
	ln -s "$(BINDIR)/zwt" "$(LINUX_USER_ZWT_PATH)"
else
	$(RM) "$(LINUX_USER_ZWT_PATH)"
endif
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
	# Reinstalling must not erase the profile action block generated by a running Zetta.
	mkdir -p "$(LINUX_USER_DESKTOP_DIR)"
	desktop_entry="$(LINUX_USER_DESKTOP_DIR)/$(APP_ID).desktop"; desktop_source="resources/linux/$(APP_ID).desktop"; if test -f "$$desktop_entry"; then if grep -Fqx "# ZETTA MANAGED PROFILE ACTIONS BEGIN" "$$desktop_entry"; then if grep -Fqx "# ZETTA MANAGED PROFILE GROUPS BEGIN" "$$desktop_entry"; then desktop_source="$$desktop_entry"; fi; fi; fi; desktop_tmp="$$desktop_entry.tmp.$$$$"; if sed -e "s|^TryExec=.*|TryExec=$(BINDIR)/zetta|" -e "1,/^\[Desktop Action / s|^Exec=[^[:space:]]*\(.*\)$$|Exec=$(BINDIR)/zetta\1|" -e "s|^Icon=.*|Icon=$(ICON_512_DIR)/$(APP_ID).png|" "$$desktop_source" > "$$desktop_tmp"; then if test -f "$$desktop_entry" && cmp -s "$$desktop_tmp" "$$desktop_entry"; then rm -f "$$desktop_tmp"; chmod 644 "$$desktop_entry"; elif chmod 644 "$$desktop_tmp"; then if mv -f "$$desktop_tmp" "$$desktop_entry"; then :; else rm -f "$$desktop_tmp"; exit 1; fi; else rm -f "$$desktop_tmp"; exit 1; fi; else rm -f "$$desktop_tmp"; exit 1; fi
endif
	$(MAKE) refresh-desktop-caches

uninstall:
	$(MAKE) uninstall-binary
	$(MAKE) uninstall-assets

uninstall-binary:
	$(RM) $(BINDIR)/zetta
	$(RM) $(BINDIR)/zmux
	$(RM) $(BINDIR)/zwt
ifneq ($(LINUX_USER_INSTALL),)
	$(RM) "$(LINUX_USER_CLI_PATH)"
	$(RM) "$(LINUX_USER_BIN_DIR)/zmux"
	$(RM) "$(LINUX_USER_ZWT_PATH)"
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
