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

home_directory=${HOME:?HOME is required}
shell_name=$(basename "${SHELL:-/bin/sh}")
zsh_directory=${ZDOTDIR:-$home_directory}
fish_directory=${XDG_CONFIG_HOME:-"$home_directory/.config"}

managed_begin='# ZETTA MANAGED PATH BEGIN'
managed_end='# ZETTA MANAGED PATH END'
legacy_comment='# Added by Zetta to make the installed CLI available.'
legacy_export_command="export PATH=\"$path_directory:\$PATH\""
legacy_fish_command="fish_add_path -m \"$path_directory\""

# Keep the path as a shell word in the generated block. Install paths normally
# contain no quoting characters, but this also keeps spaces and apostrophes
# from turning the startup file into invalid shell code.
shell_quote() {
    quoted_value=$(printf '%s' "$1" | sed "s/'/'\\\\''/g")
    printf "'%s'" "$quoted_value"
}

path_literal=$(shell_quote "$path_directory")

write_managed_block() {
    printf '%s\n' "$managed_begin"
    if [ "$shell_name" = fish ]; then
        printf 'fish_add_path --path %s\n' "$path_literal"
    else
        printf 'case ":${PATH-}:" in\n'
        printf '    *:%s:*) ;;\n' "$path_literal"
        printf '    *) export PATH=%s:"${PATH-}" ;;\n' "$path_literal"
        printf 'esac\n'
    fi
    printf '%s\n' "$managed_end"
}

# Remove only complete marker-delimited blocks and the two-line block written
# by older versions of this helper. Output is written to a temporary file and
# copied back through the original path, so a configuration symlink survives.
filter_managed_content() {
    input_path=$1
    output_path=$2
    awk \
        -v managed_begin="$managed_begin" \
        -v managed_end="$managed_end" \
        -v legacy_comment="$legacy_comment" \
        -v legacy_export_command="$legacy_export_command" \
        -v legacy_fish_command="$legacy_fish_command" '
        function print_pending() {
            if (pending) {
                print pending_line
                pending = 0
            }
        }

        function print_buffered_block() {
            for (line_index = 1; line_index <= block_line_count; line_index++) {
                print block_lines[line_index]
            }
            block_line_count = 0
        }

        {
            if (in_managed_block) {
                block_lines[++block_line_count] = $0
                if ($0 == managed_end) {
                    in_managed_block = 0
                    block_line_count = 0
                }
                next
            }

            if ($0 == managed_begin) {
                in_managed_block = 1
                block_lines[++block_line_count] = $0
                next
            }

            if (pending) {
                if ($0 == legacy_export_command || $0 == legacy_fish_command) {
                    pending = 0
                    next
                }
                print_pending()
            }

            if ($0 == legacy_comment) {
                pending_line = $0
                pending = 1
                next
            }

            print
        }

        END {
            if (in_managed_block) {
                print_buffered_block()
            }
            print_pending()
        }
    ' "$input_path" > "$output_path"
}

file_needs_filtering() {
    file_path=$1
    if grep -Fqx "$managed_begin" "$file_path" 2>/dev/null; then
        return 0
    fi
    awk \
        -v legacy_comment="$legacy_comment" \
        -v legacy_export_command="$legacy_export_command" \
        -v legacy_fish_command="$legacy_fish_command" '
        {
            if (previous == legacy_comment &&
                ($0 == legacy_export_command || $0 == legacy_fish_command)) {
                found = 1
            }
            previous = $0
        }
        END {
            exit(found ? 0 : 1)
        }
    ' "$file_path"
}

temporary_input=
temporary_output=
cleanup_temporary_files() {
    if [ -n "$temporary_input" ]; then
        rm -f "$temporary_input"
    fi
    if [ -n "$temporary_output" ]; then
        rm -f "$temporary_output"
    fi
}
trap cleanup_temporary_files EXIT HUP INT TERM

clean_file() {
    file_path=$1
    if [ ! -f "$file_path" ]; then
        return 0
    fi
    if ! file_needs_filtering "$file_path"; then
        return 0
    fi

    file_directory=$(dirname "$file_path")
    temporary_input=$(mktemp "$file_directory/.zetta-path.XXXXXX")
    filter_managed_content "$file_path" "$temporary_input"
    if ! cmp -s "$temporary_input" "$file_path"; then
        # Redirection follows a symlink at file_path; do not replace it with mv.
        cat "$temporary_input" > "$file_path"
    fi
    rm -f "$temporary_input"
    temporary_input=
}

install_block() {
    file_path=$1
    placement=$2

    file_directory=$(dirname "$file_path")
    mkdir -p "$file_directory"
    temporary_input=$(mktemp "$file_directory/.zetta-path.XXXXXX")
    temporary_output=$(mktemp "$file_directory/.zetta-path.XXXXXX")

    if [ -f "$file_path" ]; then
        if file_needs_filtering "$file_path"; then
            filter_managed_content "$file_path" "$temporary_input"
        else
            cat "$file_path" > "$temporary_input"
        fi
    else
        : > "$temporary_input"
    fi

    case "$placement" in
        prepend)
            write_managed_block > "$temporary_output"
            cat "$temporary_input" >> "$temporary_output"
            ;;
        append)
            cat "$temporary_input" > "$temporary_output"
            if [ -s "$temporary_input" ] &&
                [ "$(tail -c 1 "$temporary_input" | wc -l)" -eq 0 ]; then
                printf '\n' >> "$temporary_output"
            fi
            write_managed_block >> "$temporary_output"
            ;;
        *)
            echo "unknown block placement: $placement" >&2
            exit 1
            ;;
    esac

    # Redirection follows a symlink at file_path; do not replace it with mv.
    cat "$temporary_output" > "$file_path"
    rm -f "$temporary_input" "$temporary_output"
    temporary_input=
    temporary_output=
}

first_bash_login_file() {
    for candidate in \
        "$home_directory/.bash_profile" \
        "$home_directory/.bash_login" \
        "$home_directory/.profile"; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    printf '%s\n' "$home_directory/.profile"
}

all_managed_files() {
    printf '%s\n' \
        "$home_directory/.bashrc" \
        "$home_directory/.bash_profile" \
        "$home_directory/.bash_login" \
        "$home_directory/.profile" \
        "$home_directory/.zshenv" \
        "$zsh_directory/.zshenv" \
        "$home_directory/.zshrc" \
        "$zsh_directory/.zshrc" \
        "$home_directory/.config/fish/config.fish" \
        "$fish_directory/fish/config.fish"
}

case "$action" in
    install)
        case "$shell_name" in
            fish)
                install_block "$fish_directory/fish/config.fish" append
                printf 'Added %s to PATH in %s; open a new shell to use it.\n' \
                    "$path_directory" "$fish_directory/fish/config.fish"
                ;;
            zsh)
                # zsh reads .zshenv for both login and noninteractive shells.
                install_block "$zsh_directory/.zshenv" append
                # The old helper wrote to $HOME/.zshrc. Remove that block while
                # leaving zetta init and all other user configuration intact.
                clean_file "$home_directory/.zshrc"
                if [ "$zsh_directory/.zshrc" != "$home_directory/.zshrc" ]; then
                    clean_file "$zsh_directory/.zshrc"
                fi
                printf 'Added %s to PATH in %s; open a new shell to use it.\n' \
                    "$path_directory" "$zsh_directory/.zshenv"
                ;;
            bash)
                # BASH_ENV points at .bashrc in many SSH/automation setups, so
                # this must precede the usual interactive-only early return.
                install_block "$home_directory/.bashrc" prepend
                bash_login_file=$(first_bash_login_file)
                install_block "$bash_login_file" append
                printf 'Added %s to PATH in %s and %s; open a new shell to use it.\n' \
                    "$path_directory" "$home_directory/.bashrc" "$bash_login_file"
                ;;
            *)
                install_block "$home_directory/.profile" append
                printf 'Added %s to PATH in %s; open a new shell to use it.\n' \
                    "$path_directory" "$home_directory/.profile"
                ;;
        esac
        ;;
    uninstall)
        while IFS= read -r file_path; do
            clean_file "$file_path"
        done <<EOF
$(all_managed_files)
EOF
        ;;
    *)
        echo "unknown action: $action" >&2
        exit 1
        ;;
esac
