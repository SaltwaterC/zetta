# Zetta shell integration for Zsh.
if (( ! ${+EDITOR} )); then
    export EDITOR='zetta vi'
fi

if (( ! $+commands[vi] && ! $+aliases[vi] && ! $+functions[vi] && ! $+builtins[vi] )); then
    function vi { command zetta vi "$@"; }
    _zetta_vi_missing=1
else
    _zetta_vi_missing=0
fi

function zvi { command zetta vi "$@"; }

function zwt {
    case $1 in
        new)
            local worktree_path path_only_arg
            local -a operation_args=("${@[2,-1]}")
            for path_only_arg in "${operation_args[@]}"; do
                if [[ $path_only_arg == --path-only || $path_only_arg == -P ]]; then
                    path_only_arg=1
                    break
                fi
                path_only_arg=''
            done
            if [[ $path_only_arg == 1 ]]; then
                worktree_path=$(command zetta wt new "${operation_args[@]}") || return
            else
                worktree_path=$(command zetta wt new --path-only "${operation_args[@]}") || return
            fi
            [[ -n $worktree_path ]] || return 1
            builtin cd -- "$worktree_path"
            ;;
        done)
            local worktree_path path_only_arg
            local -a operation_args=("${@[2,-1]}")
            for path_only_arg in "${operation_args[@]}"; do
                if [[ $path_only_arg == --path-only || $path_only_arg == -P ]]; then
                    path_only_arg=1
                    break
                fi
                path_only_arg=''
            done
            if [[ $path_only_arg == 1 ]]; then
                worktree_path=$(command zetta wt done "${operation_args[@]}") || return
            else
                worktree_path=$(command zetta wt done --path-only "${operation_args[@]}") || return
            fi
            [[ -n $worktree_path ]] || return 1
            builtin cd -- "$worktree_path"
            ;;
        *)
            command zetta wt "$@"
            ;;
    esac
}

if ! (( $+functions[compdef] )); then
    autoload -Uz compinit
    compinit
fi

_zetta_option_unused() {
    local option=$1 index
    for (( index = 2; index < CURRENT; index++ )); do
        [[ ${words[index]} == "$option" ]] && return 1
    done
    return 0
}

_zetta_options() {
    local -a candidates=()
    local candidate
    for candidate in "$@"; do
        if [[ $candidate != -* ]] || _zetta_option_unused "$candidate"; then
            candidates+=("$candidate")
        fi
    done
    builtin compadd -- "${candidates[@]}"
}

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is
# removed first so Zetta's functions take priority over it. The `function
# name { ... }` form (rather than `name() { ... }`) is required here: zsh
# expands an active alias while parsing a `name() { ... }` definition of the
# same name, which fails to parse ("defining function based on alias") even
# though the preceding unalias runs first, because the whole case branch is
# parsed as one unit before any of it executes.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        function pbcopy { zetta copy "$@"; }
        function pbpaste { zetta paste "$@"; }
        ;;
esac

_zetta_profiles() {
    local -a config_args=("$@")
    compadd -- "${(@f)$(zetta profile list "${config_args[@]}" 2>/dev/null)}"
}

_zetta_profile_themes() {
    local -a config_args=("$@")
    compadd -- "${(@f)$(zetta profile themes "${config_args[@]}" 2>/dev/null)}"
}

_zetta_split_names() {
    compadd -- "${(@f)$(zetta splits 2>/dev/null)}"
}

_zetta_session_ids() {
    compadd -- "${(@f)$(zetta sessions --json 2>/dev/null | awk '
        /"process_id"[[:space:]]*:/ { match($0, /[0-9]+/); process=substr($0, RSTART, RLENGTH) }
        /"runner_id"[[:space:]]*:/ { match($0, /[0-9]+/); runner=substr($0, RSTART, RLENGTH) }
        /"id"[[:space:]]*:/ { match($0, /[0-9]+/); session=substr($0, RSTART, RLENGTH) }
        /"authentication_required"[[:space:]]*:/ { print process ":" runner ":" session }
    ')}"
}

_zetta_tab_icons() {
    compadd -- "${(@f)$(zetta tabicon --list 2>/dev/null)}"
}

_zetta_pane_themes() {
    compadd -- "${(@f)$(zetta panetheme --list 2>/dev/null)}"
}

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
_zetta_sound_names() {
    case "$OSTYPE" in
        darwin*)
            compadd -- zetta-default zetta-ok zetta-alarm \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
            ;;
        msys*|cygwin*|win32*)
            compadd -- zetta-default zetta-ok zetta-alarm Default IM Mail Reminder SMS
            ;;
        *)
            compadd -- zetta-default zetta-ok zetta-alarm bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
            ;;
    esac
}

_zetta() {
    local previous=${words[CURRENT-1]} profile_operation='' profile_command_index=-1
    local index
    local -a config_args=()

    for (( index = 2; index < CURRENT; index++ )); do
        case ${words[index]} in
            --config|-c)
                if (( index + 1 < CURRENT )); then
                    config_args+=(--config "${words[index+1]}")
                    (( index++ ))
                fi
                ;;
            --keymap|-k|--profile|-p|--split|-s|--theme|-t)
                (( index++ ))
                ;;
            profile)
                profile_command_index=$index
                break
                ;;
        esac
    done
    if (( profile_command_index >= 0 )); then
        index=$((profile_command_index + 1))
        while (( index < CURRENT )); do
            case ${words[index]} in
                --config|-c)
                    (( index += 2 ))
                    ;;
                --help|-h)
                    (( index++ ))
                    ;;
                -*)
                    break
                    ;;
                *)
                    profile_operation=${words[index]}
                    break
                    ;;
            esac
        done
    fi

    if [[ $words[1] == edit ]]; then
        if [[ $words[CURRENT] == -* ]]; then
            _zetta_options --delete-after --help
        else
            _files
        fi
        return
    fi

    if [[ $words[1] == vi || $words[1] == zvi ]]; then
        if [[ $words[CURRENT] == -* ]]; then
            _zetta_options --help
        else
            _files
        fi
        return
    fi

    if (( CURRENT == 2 )); then
        compadd -S ' ' -- benchmark benchmark-output terminal-size sessions profile edit vi init serial http tftp notify copy paste splits tabicon panetheme overlay wt
        _zetta_options --help --version --config --keymap --profile --split --replace-pane --theme
        return
    fi

    if (( profile_command_index >= 0 && CURRENT == profile_command_index + 1 )); then
        compadd -S ' ' -- list themes disable enable theme default add remove
        _zetta_options --config --help
        return
    fi

    case $previous in
        --profile)
            _zetta_profiles
            return
            ;;
        -p)
            if [[ $words[2] == profile && $profile_operation == add ]]; then
                return
            elif [[ $words[2] == serial ]]; then
                compadd -- none odd even
            elif [[ $words[2] != http && $words[2] != tftp && $words[2] != notify ]]; then
                _zetta_profiles
            fi
            return
            ;;
        --config|--keymap|-k|--profile-report)
            _files
            return
            ;;
        --program|--arg)
            return
            ;;
        --root)
            _files -/
            return
            ;;
        --device)
            compadd -- "${(@f)$(zetta serial list 2>/dev/null)}"
            return
            ;;
        -d)
            if [[ $words[2] == serial ]]; then
                compadd -- "${(@f)$(zetta serial list 2>/dev/null)}"
            fi
            return
            ;;
        --data-bits|-D)
            if [[ $words[2] == serial ]]; then
                compadd -- 5 6 7 8
            fi
            return
            ;;
        --parity)
            compadd -- none odd even
            return
            ;;
        --split)
            _zetta_split_names
            return
            ;;
        --replace-pane)
            if [[ $words[CURRENT] == -* || -z $words[CURRENT] ]]; then
                _zetta_options --help --version --config --keymap --profile --split --theme
            fi
            return
            ;;
        --stop-bits|--size)
            if [[ $words[2] == serial ]]; then
                compadd -- 1 2
            elif [[ $words[2] == notify ]]; then
                _zetta_sound_names
            elif [[ $words[2] == overlay ]]; then
                compadd -- sm base lg xl 2xl 3xl
            fi
            return
            ;;
        -s)
            if [[ $words[2] == -* || -z $words[2] ]]; then
                _zetta_split_names
            elif [[ $words[2] == serial ]]; then
                compadd -- 1 2
            elif [[ $words[2] == notify ]]; then
                _zetta_sound_names
            elif [[ $words[2] == overlay ]]; then
                compadd -- sm base lg xl 2xl 3xl
            fi
            return
            ;;
        --flow-control|-f)
            compadd -- none software hardware
            return
            ;;
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            compadd -- txt rtf ps
            return
            ;;
        --app-name|-a)
            return
            ;;
        --icon|-i)
            if [[ $words[2] == tabicon ]]; then
                _zetta_tab_icons
            else
                _files
            fi
            return
            ;;
        --sound)
            _zetta_sound_names
            return
            ;;
        --timeout)
            compadd -- default never
            return
            ;;
        --opacity|-o)
            return
            ;;
        -c)
            if [[ $words[2] == overlay ]]; then
                compadd -- ZETTA_OVERLAY_COLORS
                return
            elif [[ $words[2] == terminal-size ]]; then
                return
            fi
            _files
            return
            ;;
        --color)
            if [[ $words[2] == overlay ]]; then
                compadd -- ZETTA_OVERLAY_COLORS
            fi
            return
            ;;
        -r)
            if [[ $words[2] == http || ( $words[2] == tftp && $words[3] == server ) ]]; then
                _files -/
                return
            fi
            if [[ $words[2] == terminal-size || $words[2] == profile || $words[2] == -* || -z $words[2] ]]; then
                if [[ $words[2] == -* && ($words[CURRENT] == -* || -z $words[CURRENT]) ]]; then
                    _zetta_options --help --version --config --keymap --profile --split --theme
                fi
                return
            fi
            _files
            return
            ;;
        --output-type|-t|--theme|--text)
            if [[ $words[2] == profile || $words[2] == -* ]]; then
                _zetta_profile_themes "${config_args[@]}"
            elif [[ $words[2] == panetheme ]]; then
                _zetta_pane_themes
            elif [[ $words[2] == notify ]]; then
                compadd -- default never
            elif [[ $words[2] == overlay ]]; then
                return
            else
                compadd -- repeated unique
            fi
            return
            ;;
        --port|-p|--baud-rate|-b|--profile-duration|--columns|--rows|-R)
            return
            ;;
    esac

    if [[ -n $profile_operation ]]; then
        case $profile_operation in
            list|themes)
                _zetta_options --config --help
                ;;
            disable|enable|default|remove)
                if [[ $previous == "$profile_operation" ]]; then
                    _zetta_profiles "${config_args[@]}"
                else
                    _zetta_options --config --help
                fi
                ;;
            theme)
                if [[ $words[CURRENT] == -* ]]; then
                    _zetta_options --reset --config --help
                elif [[ $previous == theme ]]; then
                    _zetta_profiles "${config_args[@]}"
                elif [[ $previous == --reset || $previous == -r ]]; then
                    _zetta_options --config --help
                else
                    _zetta_profile_themes "${config_args[@]}"
                fi
                ;;
            add)
                _zetta_options --program --arg --theme --config --help
                ;;
            *)
                _zetta_options list themes disable enable theme default add remove --config --help
                ;;
        esac
        return
    fi

    # A leading flag rules out a subcommand for the rest of the command line
    # (subcommands are only recognized as the first argument), so keep
    # offering the remaining top-level flags instead of falling through to
    # the subcommand-specific cases below, which would offer nothing.
    if [[ $words[2] == -* ]]; then
        _zetta_options --help --version --config --keymap --profile --split --replace-pane --theme
        return
    fi

    case $words[2] in
        benchmark)
            _zetta_options --terminal-render-workload --terminal-checkerboard-workload \
                --terminal-sparse-update-workload --profile-report --profile-duration \
                --profile-pane-stress --profile-background-stress --profile-sparse-updates \
                --profile-external-terminal --help
            ;;
        benchmark-output)
            _zetta_options --size --output-type --help
            ;;
        terminal-size)
            _zetta_options --json --resize --columns --rows --help
            ;;
        edit)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --delete-after --help
            else
                _files
            fi
            ;;
        vi)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --help
            else
                _files
            fi
            ;;
        sessions)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- reconnect
                _zetta_options --json --help
            elif [[ $words[3] == reconnect ]]; then
                if [[ $previous != --session && $previous != -s ]]; then
                    if (( CURRENT == 4 )); then
                        _zetta_session_ids
                    else
                        _zetta_options --session --help
                    fi
                fi
            else
                _zetta_options --json --help
            fi
            ;;
        init)
            compadd -- bash fish powershell pwsh zsh --help
            ;;
        serial)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- console list
                _zetta_options --help
            elif [[ $words[3] == console ]]; then
                _zetta_options --device --baud-rate --data-bits --parity --stop-bits --flow-control --help
            fi
            ;;
        http)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- server
                _zetta_options --help
            else
                _zetta_options --root --port --config --help
            fi
            ;;
        tftp)
            _zetta_tftp
            ;;
        notify)
            _zetta_options --app-name --icon --sound --timeout --help
            ;;
        copy)
            _zetta_options --pboard --help
            ;;
        paste)
            _zetta_options --pboard --prefer --help
            ;;
        splits)
            _zetta_options --help
            ;;
        tabicon)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --icon --list --help
            else
                _zetta_tab_icons
            fi
            ;;
        panetheme)
            if [[ $words[CURRENT] == -* ]]; then
                _zetta_options --theme --reset --list --help
            else
                _zetta_pane_themes
            fi
            ;;
        overlay)
            _zetta_options --text --size --opacity --color --reset --help
            ;;
        wt)
            if (( CURRENT == 3 )); then
                compadd -S ' ' -- new done status rerere
                _zetta_options --help
            elif [[ $words[3] == new || $words[3] == done ]]; then
                _zetta_options --path-only --help
            else
                _zetta_options --help
            fi
            ;;
    esac
}

_zetta_tftp() {
    local operation_index operation position=0 index argument skip_port=0
    local current=${words[CURRENT]}

    if [[ $words[1] == ztftp ]]; then
        operation_index=2
    else
        operation_index=3
    fi

    if (( CURRENT == operation_index )); then
        compadd -S ' ' -- get put server
        _zetta_options --help
        return
    fi

    operation=${words[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            _zetta_options --root --port --config --help
        fi
        return
    fi

    if [[ $current == -* ]]; then
        _zetta_options --port --help
        return
    fi
    if [[ $words[CURRENT-1] == --port || $words[CURRENT-1] == -p ]]; then
        return
    fi

    for (( index = operation_index + 1; index < CURRENT; index++ )); do
        argument=${words[index]}
        if (( skip_port )); then
            skip_port=0
        elif [[ $argument == --port || $argument == -p ]]; then
            skip_port=1
        elif [[ $argument != -* ]]; then
            (( position++ ))
        fi
    done

    case $operation in
        put)
            (( position == 1 )) && _files
            ;;
    esac
}

_ztftp() {
    _zetta_tftp
}

_zntfy() {
    local previous=${words[CURRENT-1]}

    case $previous in
        --app-name|-a)
            return
            ;;
        --icon|-i)
            _files
            return
            ;;
        --sound|-s)
            _zetta_sound_names
            return
            ;;
        --timeout|-t)
            compadd -- default never
            return
            ;;
    esac
    _zetta_options --app-name --icon --sound --timeout --help
}

_zcopy() {
    local previous=${words[CURRENT-1]}
    case $previous in
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
    esac
    _zetta_options --pboard --help
}

_zpaste() {
    local previous=${words[CURRENT-1]}
    case $previous in
        --pboard|-pboard)
            compadd -- general ruler find font
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            compadd -- txt rtf ps
            return
            ;;
    esac
    _zetta_options --pboard --prefer --help
}

compdef _zetta zetta
_zwt() {
    local -a saved_words=("${words[@]}")
    local saved_current=$CURRENT
    words=(zetta wt "${words[@]:1}")
    (( CURRENT++ ))
    _zetta
    words=("${saved_words[@]}")
    CURRENT=$saved_current
}
compdef _zwt zwt
compdef _ztftp ztftp
compdef _zntfy zntfy
compdef _zcopy zcopy
compdef _zpaste zpaste
compdef _zetta zvi
if (( _zetta_vi_missing )); then
    compdef _zetta vi
fi
case "$OSTYPE" in
    darwin*) ;;
    *)
        compdef _zcopy pbcopy
        compdef _zpaste pbpaste
        ;;
esac
