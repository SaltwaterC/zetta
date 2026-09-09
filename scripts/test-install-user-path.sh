#!/usr/bin/env sh

set -eu

script_directory=$(dirname "$0")
installer=$script_directory/install-user-path.sh
test_root=$(mktemp -d "${TMPDIR:-/tmp}/zetta-install-user-path.XXXXXX")

cleanup() {
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

fail() {
    printf 'installer test failed: %s\n' "$1" >&2
    exit 1
}

assert_true() {
    if ! "$@"; then
        fail "assertion failed: $*"
    fi
}

assert_file_contains() {
    file_path=$1
    expected=$2
    message=$3
    if ! grep -Fqx "$expected" "$file_path"; then
        fail "$message"
    fi
}

assert_file_has_text() {
    file_path=$1
    expected=$2
    message=$3
    if ! grep -Fq "$expected" "$file_path"; then
        fail "$message"
    fi
}

assert_file_lacks_text() {
    file_path=$1
    unexpected=$2
    message=$3
    if grep -Fq "$unexpected" "$file_path"; then
        fail "$message"
    fi
}

assert_path_result() {
    actual=$1
    expected=$2
    message=$3
    if [ "$actual" != "$expected" ]; then
        fail "$message (expected '$expected', got '$actual')"
    fi
}

assert_path_once() {
    path_value=$1
    expected_entry=$2
    message=$3
    entry_count=$(printf '%s\n' "$path_value" | tr ':' '\n' | grep -Fxc "$expected_entry" || true)
    if [ "$entry_count" -ne 1 ]; then
        fail "$message (found $entry_count entries)"
    fi
}

make_tool() {
    tool_directory=$1
    mkdir -p "$tool_directory"
    printf '#!/bin/sh\nexit 0\n' > "$tool_directory/zosh-server"
    chmod 755 "$tool_directory/zosh-server"
}

install_legacy_export() {
    file_path=$1
    path_value=$2
    printf '%s\n' '# Added by Zetta to make the installed CLI available.' >> "$file_path"
    printf 'export PATH="%s:$PATH"\n' "$path_value" >> "$file_path"
}

install_legacy_fish() {
    file_path=$1
    path_value=$2
    printf '%s\n' '# Added by Zetta to make the installed CLI available.' >> "$file_path"
    printf 'fish_add_path -m "%s"\n' "$path_value" >> "$file_path"
}

test_zsh() {
    zsh_home=$test_root/zsh-home
    zsh_directory=$zsh_home/zsh-config
    zsh_bin=$zsh_home/.local/bin
    zshenv_target=$zsh_home/zshenv-target
    mkdir -p "$zsh_directory"
    make_tool "$zsh_bin"

    printf '%s\n' '# zsh user configuration' 'eval "$(zetta init zsh)"' > "$zsh_home/.zshrc"
    install_legacy_export "$zsh_home/.zshrc" "$zsh_bin"
    printf '%s\n' '# zshenv user configuration' > "$zshenv_target"
    ln -s "$zshenv_target" "$zsh_directory/.zshenv"

    HOME="$zsh_home" ZDOTDIR="$zsh_directory" SHELL=/bin/zsh \
        sh "$installer" "$zsh_bin"
    assert_true test -L "$zsh_directory/.zshenv"
    assert_file_contains "$zshenv_target" '# ZETTA MANAGED PATH BEGIN' \
        'zsh managed block was not installed in ZDOTDIR/.zshenv'
    assert_file_contains "$zshenv_target" '# ZETTA MANAGED PATH END' \
        'zsh managed block has no end marker'
    assert_file_has_text "$zsh_home/.zshrc" 'eval "$(zetta init zsh)"' \
        'zsh init was removed during legacy migration'
    assert_file_lacks_text "$zsh_home/.zshrc" \
        '# Added by Zetta to make the installed CLI available.' \
        'legacy zsh PATH marker was not migrated'
    zsh_result=$(env -i HOME="$zsh_home" ZDOTDIR="$zsh_directory" \
        PATH=/usr/bin:/bin zsh -c 'command -v zosh-server')
    assert_path_result "$zsh_result" "$zsh_bin/zosh-server" \
        'zsh noninteractive commands cannot find zosh-server'

    zshenv_before=$(cksum "$zshenv_target")
    zshrc_before=$(cksum "$zsh_home/.zshrc")
    HOME="$zsh_home" ZDOTDIR="$zsh_directory" SHELL=/bin/zsh \
        sh "$installer" "$zsh_bin" >/dev/null
    assert_path_result "$(cksum "$zshenv_target")" "$zshenv_before" \
        'reinstall duplicated or changed the zsh managed block'
    assert_path_result "$(cksum "$zsh_home/.zshrc")" "$zshrc_before" \
        'reinstall changed migrated zsh configuration'

    HOME="$zsh_home" ZDOTDIR="$zsh_directory" SHELL=/bin/zsh \
        sh "$installer" "$zsh_bin" uninstall
    assert_true test -L "$zsh_directory/.zshenv"
    assert_file_has_text "$zshenv_target" '# zshenv user configuration' \
        'zsh uninstall removed unrelated configuration'
    assert_file_lacks_text "$zshenv_target" '# ZETTA MANAGED PATH BEGIN' \
        'zsh uninstall left the managed block'
    assert_file_has_text "$zsh_home/.zshrc" 'eval "$(zetta init zsh)"' \
        'zsh uninstall removed zetta init'
}

test_bash() {
    bash_home=$test_root/bash-home
    bash_bin=$bash_home/.local/bin
    bashrc_target=$bash_home/bashrc-target
    mkdir -p "$bash_home"
    make_tool "$bash_bin"

    printf '%s\n' '# bash user configuration' 'export PATH="/user/bin:$PATH"' \
        'case $- in' '    *i*) ;;' '    *) return;;' 'esac' > "$bashrc_target"
    install_legacy_export "$bashrc_target" "$bash_bin"
    ln -s "$bashrc_target" "$bash_home/.bashrc"
    printf '%s\n' '# bash login configuration' \
        'if [ -f "$HOME/.bashrc" ]; then . "$HOME/.bashrc"; fi' > \
        "$bash_home/.bash_profile"
    printf '%s\n' '# lower-priority bash login file' > "$bash_home/.bash_login"
    printf '%s\n' '# fallback profile must remain untouched' > "$bash_home/.profile"

    HOME="$bash_home" SHELL=/bin/bash sh "$installer" "$bash_bin"
    assert_true test -L "$bash_home/.bashrc"
    assert_file_contains "$bashrc_target" '# ZETTA MANAGED PATH BEGIN' \
        'bash managed block was not installed'
    assert_file_has_text "$bashrc_target" 'export PATH="/user/bin:$PATH"' \
        'bash user PATH configuration was removed'
    assert_file_lacks_text "$bashrc_target" \
        '# Added by Zetta to make the installed CLI available.' \
        'legacy bash PATH marker was not migrated'
    assert_file_contains "$bash_home/.bash_profile" '# ZETTA MANAGED PATH BEGIN' \
        'bash login managed block was not installed'
    assert_file_lacks_text "$bash_home/.bash_login" '# ZETTA MANAGED PATH BEGIN' \
        'bash installer ignored first applicable login file'
    assert_file_lacks_text "$bash_home/.profile" '# ZETTA MANAGED PATH BEGIN' \
        'bash installer changed a lower-priority login file'

    bash_env_path=$(env -i HOME="$bash_home" PATH=/usr/bin:/bin \
        BASH_ENV="$bash_home/.bashrc" bash -c 'printf "%s\n" "$PATH"')
    assert_path_once "$bash_env_path" "$bash_bin" \
        'BASH_ENV startup did not add exactly one installed PATH entry'
    bash_login_path=$(env -i HOME="$bash_home" PATH=/usr/bin:/bin \
        bash --login -c 'printf "%s\n" "$PATH"')
    assert_path_once "$bash_login_path" "$bash_bin" \
        'Bash login startup did not add exactly one installed PATH entry'
    bash_command=$(env -i HOME="$bash_home" PATH=/usr/bin:/bin \
        BASH_ENV="$bash_home/.bashrc" bash -c 'command -v zosh-server')
    assert_path_result "$bash_command" "$bash_bin/zosh-server" \
        'BASH_ENV commands cannot find zosh-server'

    bashrc_before=$(cksum "$bashrc_target")
    bash_profile_before=$(cksum "$bash_home/.bash_profile")
    HOME="$bash_home" SHELL=/bin/bash sh "$installer" "$bash_bin" >/dev/null
    assert_path_result "$(cksum "$bashrc_target")" "$bashrc_before" \
        'reinstall duplicated or moved the bashrc managed block'
    assert_path_result "$(cksum "$bash_home/.bash_profile")" "$bash_profile_before" \
        'reinstall changed the bash login file'

    HOME="$bash_home" SHELL=/bin/bash sh "$installer" "$bash_bin" uninstall
    assert_true test -L "$bash_home/.bashrc"
    assert_file_has_text "$bashrc_target" 'export PATH="/user/bin:$PATH"' \
        'bash uninstall removed unrelated PATH configuration'
    assert_file_lacks_text "$bashrc_target" '# ZETTA MANAGED PATH BEGIN' \
        'bash uninstall left the bashrc managed block'
    assert_file_has_text "$bash_home/.bash_profile" \
        'if [ -f "$HOME/.bashrc" ]; then . "$HOME/.bashrc"; fi' \
        'bash uninstall removed login configuration'
}

test_fish() {
    fish_home=$test_root/fish-home
    fish_bin=$fish_home/.local/bin
    fish_config_directory=$fish_home/.config/fish
    fish_config_target=$fish_home/fish-config-target
    mkdir -p "$fish_config_directory"
    make_tool "$fish_bin"

    printf '%s\n' '# fish user configuration' > "$fish_config_target"
    install_legacy_fish "$fish_config_target" "$fish_bin"
    ln -s "$fish_config_target" "$fish_config_directory/config.fish"

    HOME="$fish_home" SHELL=/usr/bin/fish sh "$installer" "$fish_bin"
    assert_true test -L "$fish_config_directory/config.fish"
    assert_file_contains "$fish_config_target" '# ZETTA MANAGED PATH BEGIN' \
        'fish managed block was not installed'
    assert_file_has_text "$fish_config_target" 'fish_add_path --path' \
        'fish installer did not use fish_add_path --path'
    assert_file_lacks_text "$fish_config_target" 'fish_add_path -m' \
        'legacy fish PATH command was not migrated'
    fish_command=$(env -i HOME="$fish_home" PATH=/usr/bin:/bin \
        fish -c 'command -v zosh-server')
    assert_path_result "$fish_command" "$fish_bin/zosh-server" \
        'fish commands cannot find zosh-server'

    fish_config_before=$(cksum "$fish_config_target")
    HOME="$fish_home" SHELL=/usr/bin/fish sh "$installer" "$fish_bin" >/dev/null
    assert_path_result "$(cksum "$fish_config_target")" "$fish_config_before" \
        'reinstall duplicated the fish managed block'

    HOME="$fish_home" SHELL=/usr/bin/fish sh "$installer" "$fish_bin" uninstall
    assert_true test -L "$fish_config_directory/config.fish"
    assert_file_has_text "$fish_config_target" '# fish user configuration' \
        'fish uninstall removed unrelated configuration'
    assert_file_lacks_text "$fish_config_target" '# ZETTA MANAGED PATH BEGIN' \
        'fish uninstall left the managed block'
}

test_posix_login() {
    posix_home=$test_root/posix-home
    posix_bin=$posix_home/.local/bin
    mkdir -p "$posix_home"
    make_tool "$posix_bin"
    printf '%s\n' '# POSIX user profile' 'export POSIX_USER_SETTING=kept' > \
        "$posix_home/.profile"

    HOME="$posix_home" SHELL=/bin/other-posix sh "$installer" "$posix_bin"
    posix_command=$(env -i HOME="$posix_home" PATH=/usr/bin:/bin \
        sh -l -c 'command -v zosh-server')
    assert_path_result "$posix_command" "$posix_bin/zosh-server" \
        'fallback POSIX login shell cannot find zosh-server'
    assert_file_contains "$posix_home/.profile" '# ZETTA MANAGED PATH BEGIN' \
        'fallback POSIX profile was not updated'

    HOME="$posix_home" SHELL=/bin/other-posix sh "$installer" "$posix_bin" uninstall
    assert_file_has_text "$posix_home/.profile" \
        'export POSIX_USER_SETTING=kept' \
        'POSIX uninstall removed unrelated profile configuration'
    assert_file_lacks_text "$posix_home/.profile" '# ZETTA MANAGED PATH BEGIN' \
        'POSIX uninstall left the managed block'
}

if command -v zsh >/dev/null 2>&1; then
    test_zsh
else
    printf 'Skipping zsh installer coverage: zsh is not installed.\n'
fi

test_bash

if command -v fish >/dev/null 2>&1; then
    test_fish
else
    printf 'Skipping fish installer coverage: fish is not installed.\n'
fi

test_posix_login
printf 'Unix installer tests passed.\n'
