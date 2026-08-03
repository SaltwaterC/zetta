#!/usr/bin/env sh

set -eu

if [ "$#" -lt 1 ]; then
    echo "user-local binary directory is required" >&2
    exit 1
fi

path_directory=$1
action=install
if [ "$#" -ge 2 ]; then
    action=$2
fi
shell_name=$(basename "${SHELL:-/bin/sh}")

case "$shell_name" in
    fish)
        config_path="$HOME/.config/fish/config.fish"
        path_command="fish_add_path -m \"$path_directory\""
        ;;
    zsh)
        config_path="$HOME/.zshrc"
        path_command="export PATH=\"$path_directory:\$PATH\""
        ;;
    bash)
        config_path="$HOME/.bashrc"
        path_command="export PATH=\"$path_directory:\$PATH\""
        ;;
    *)
        config_path="$HOME/.profile"
        path_command="export PATH=\"$path_directory:\$PATH\""
        ;;
esac

case "$action" in
    install)
        if ! grep -Fqx "$path_command" "$config_path" 2>/dev/null; then
            mkdir -p "$(dirname "$config_path")"
            {
                printf '\n# Added by Zetta to make the installed CLI available.\n'
                printf '%s\n' "$path_command"
            } >> "$config_path"
            printf 'Added %s to PATH in %s; open a new shell to use it.\n' \
                "$path_directory" "$config_path"
        fi
        ;;
    uninstall)
        if [ -f "$config_path" ] && grep -Fqx "$path_command" "$config_path"; then
            temporary_path="$config_path.zetta.$$"
            temporary_clean_path="$temporary_path.clean"
            trap 'rm -f "$temporary_path" "$temporary_clean_path"' EXIT HUP INT TERM
            grep -Fvx "$path_command" "$config_path" > "$temporary_path" || true
            grep -Fvx '# Added by Zetta to make the installed CLI available.' \
                "$temporary_path" > "$temporary_clean_path" || true
            mv "$temporary_clean_path" "$config_path"
            rm -f "$temporary_path"
            printf 'Removed %s from PATH in %s.\n' "$path_directory" "$config_path"
        fi
        ;;
    *)
        echo "unknown action: $action" >&2
        exit 1
        ;;
esac
