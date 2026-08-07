# Zetta shell integration for Bash.
if [[ -z ${EDITOR+x} ]]; then
    export EDITOR='zetta vi'
fi

if ! type -t vi >/dev/null 2>&1; then
    eval 'vi() { command zetta vi "$@"; }'
    complete -F _zetta_complete vi
fi

zvi() { command zetta vi "$@"; }

_zetta_option_used() {
    local option=$1 index
    for (( index = 1; index < COMP_CWORD; index++ )); do
        [[ ${COMP_WORDS[index]} == "$option" ]] && return 0
    done
    return 1
}

_zetta_compgen() {
    local options=$1 candidate
    local -a available=()
    for candidate in $options; do
        if [[ $candidate != -* ]] || ! _zetta_option_used "$candidate"; then
            available+=("$candidate")
        fi
    done
    COMPREPLY=( $(compgen -W "${available[*]}" -- "$current") )
}

_zetta_complete() {
    local current previous command
    local -a profiles=(ZETTA_PROFILES)
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}
    command=${COMP_WORDS[1]}

    if [[ ${COMP_WORDS[0]} == vi || ${COMP_WORDS[0]} == zvi ]]; then
        if [[ $current == -* ]]; then
            _zetta_compgen '--help'
        else
            COMPREPLY=( $(compgen -f -- "$current") )
        fi
        return
    fi

    _zetta_complete_profiles() {
        COMPREPLY=()
        local profile
        for profile in "${profiles[@]}"; do
            [[ $profile == "$current"* ]] && COMPREPLY+=("$profile")
        done
    }

    _zetta_complete_session_ids() {
        COMPREPLY=()
        local session_id
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <(zetta sessions --json 2>/dev/null | awk '
            /"process_id"[[:space:]]*:/ { match($0, /[0-9]+/); process=substr($0, RSTART, RLENGTH) }
            /"runner_id"[[:space:]]*:/ { match($0, /[0-9]+/); runner=substr($0, RSTART, RLENGTH) }
            /"id"[[:space:]]*:/ { match($0, /[0-9]+/); session=substr($0, RSTART, RLENGTH) }
            /"authentication_required"[[:space:]]*:/ { print process ":" runner ":" session }
        ')
    }

    _zetta_complete_tab_icons() {
        local icons
        icons=$(zetta tabicon --list 2>/dev/null)
        COMPREPLY=( $(compgen -W "$icons" -- "$current") )
    }

    _zetta_complete_pane_themes() {
        COMPREPLY=()
        local theme
        while IFS= read -r theme; do
            [[ $theme == "$current"* ]] && COMPREPLY+=("$theme")
        done < <(zetta panetheme --list 2>/dev/null)
    }

    case "$previous" in
        --profile)
            _zetta_complete_profiles
            return
            ;;
        -p)
            if [[ $command == serial ]]; then
                _zetta_compgen 'none odd even'
            elif [[ $command != tftp && $command != http && $command != notify ]]; then
                _zetta_complete_profiles
            else
                COMPREPLY=()
            fi
            return
            ;;
        --root)
            COMPREPLY=( $(compgen -d -- "$current") )
            return
            ;;
        --device)
            _zetta_complete_serial_devices
            return
            ;;
        -d)
            if [[ $command == serial ]]; then
                _zetta_complete_serial_devices
            else
                COMPREPLY=()
            fi
            return
            ;;
        --data-bits|-D)
            if [[ $command == serial ]]; then
                _zetta_compgen '5 6 7 8'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --parity)
            _zetta_compgen 'none odd even'
            return
            ;;
        --stop-bits|-s|--size)
            if [[ $command == serial ]]; then
                _zetta_compgen '1 2'
            elif [[ $command == notify ]]; then
                _zetta_complete_sound_names
            elif [[ $command == overlay ]]; then
                _zetta_compgen 'sm base lg xl 2xl 3xl'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --flow-control|-f)
            _zetta_compgen 'none software hardware'
            return
            ;;
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            _zetta_compgen 'txt rtf ps'
            return
            ;;
        --app-name|-a)
            COMPREPLY=()
            return
            ;;
        --icon|-i)
            if [[ $command == tabicon ]]; then
                _zetta_complete_tab_icons
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --sound)
            _zetta_complete_sound_names
            return
            ;;
        --timeout)
            _zetta_compgen 'default never'
            return
            ;;
        --opacity|-o)
            COMPREPLY=()
            return
            ;;
        --config|--keymap|-k|--profile-report)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        -c)
            if [[ $command == terminal-size || $command == overlay ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --color)
            COMPREPLY=()
            return
            ;;
        -r)
            if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then
                COMPREPLY=( $(compgen -d -- "$current") )
            elif [[ $command == terminal-size ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --output-type|-t|--theme|--text)
            if [[ $command == panetheme || $command == -* ]]; then
                _zetta_complete_pane_themes
            elif [[ $command == notify ]]; then
                _zetta_compgen 'default never'
            elif [[ $command == overlay ]]; then
                COMPREPLY=()
            else
                _zetta_compgen 'repeated unique'
            fi
            return
            ;;
        --port|-p|--baud-rate|-b|--profile-duration|--columns|--rows|-R)
            COMPREPLY=()
            return
            ;;
    esac

    if (( COMP_CWORD == 1 )); then
        _zetta_compgen 'benchmark benchmark-output terminal-size sessions edit vi init serial http tftp notify copy paste tabicon panetheme overlay --help --version --config --keymap --profile --theme'
        return
    fi

    # A leading flag rules out a subcommand for the rest of the command line
    # (subcommands are only recognized as the first argument), so keep
    # offering the remaining top-level flags instead of falling through to
    # the subcommand-specific cases below, which would offer nothing.
    if [[ $command == -* ]]; then
        _zetta_compgen '--help --version --config --keymap --profile --theme'
        return
    fi

    case "$command" in
        benchmark)
            _zetta_compgen '--terminal-render-workload --terminal-checkerboard-workload --terminal-sparse-update-workload --profile-report --profile-duration --profile-pane-stress --profile-background-stress --profile-sparse-updates --profile-external-terminal --help'
            ;;
        benchmark-output)
            _zetta_compgen '--size --output-type --help'
            ;;
        terminal-size)
            _zetta_compgen '--json --resize --columns --rows --help'
            ;;
        edit)
            if [[ $current == -* ]]; then
                _zetta_compgen '--delete-after --help'
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            ;;
        vi)
            if [[ $current == -* ]]; then
                _zetta_compgen '--help'
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            ;;
        sessions)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'reconnect --json --help'
            elif [[ ${COMP_WORDS[2]} == reconnect ]]; then
                if [[ $previous == --session || $previous == -s ]]; then
                    COMPREPLY=()
                elif (( COMP_CWORD == 3 )); then
                    _zetta_complete_session_ids
                else
                    _zetta_compgen '--session --help'
                fi
            else
                _zetta_compgen '--json --help'
            fi
            ;;
        init)
            _zetta_compgen 'bash fish powershell pwsh zsh --help'
            ;;
        serial)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'console list --help'
            elif [[ ${COMP_WORDS[2]} == console ]]; then
                _zetta_compgen '--device --baud-rate --data-bits --parity --stop-bits --flow-control --help'
            fi
            ;;
        http)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'server --help'
            else
                _zetta_compgen '--root --port --config --help'
            fi
            ;;
        tftp)
            _zetta_tftp_complete 2
            ;;
        notify)
            _zetta_compgen '--app-name --icon --sound --timeout --help'
            ;;
        copy)
            _zetta_compgen '--pboard --help'
            ;;
        paste)
            _zetta_compgen '--pboard --prefer --help'
            ;;
        tabicon)
            if [[ $current == -* ]]; then
                _zetta_compgen '--icon --list --help'
            else
                _zetta_complete_tab_icons
            fi
            ;;
        panetheme)
            if [[ $current == -* ]]; then
                _zetta_compgen '--theme --reset --list --help'
            else
                _zetta_complete_pane_themes
            fi
            ;;
        overlay)
            _zetta_compgen '--text --size --opacity --color --reset --help'
            ;;
    esac
}

_zetta_tftp_complete() {
    local operation_index=$1 current previous operation argument
    local index positional=0 skip_port=0
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    if (( COMP_CWORD == operation_index )); then
        _zetta_compgen 'get put server --help'
        return
    fi
    operation=${COMP_WORDS[operation_index]}
    if [[ $operation == server ]]; then
        if [[ $current == -* || -z $current ]]; then
            _zetta_compgen '--root --port --config --help'
        fi
        return
    fi
    if [[ $current == -* ]]; then
        _zetta_compgen '--port --help'
        return
    fi
    if [[ $previous == '--port' || $previous == '-p' ]]; then
        COMPREPLY=()
        return
    fi

    for (( index = operation_index + 1; index < COMP_CWORD; index++ )); do
        argument=${COMP_WORDS[index]}
        if (( skip_port )); then
            skip_port=0
        elif [[ $argument == '--port' || $argument == '-p' ]]; then
            skip_port=1
        elif [[ $argument != -* ]]; then
            (( positional++ ))
        fi
    done

    case $operation in
        put)
            (( positional == 1 )) && COMPREPLY=( $(compgen -f -- "$current") )
            ;;
    esac
}

_zetta_complete_serial_devices() {
    local devices
    devices=$(zetta serial list 2>/dev/null)
    COMPREPLY=( $(compgen -W "$devices" -- "$current") )
}

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
_zetta_complete_sound_names() {
    local platform_sounds
    case "$OSTYPE" in
        darwin*)
            platform_sounds='Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink'
            ;;
        msys*|cygwin*|win32*)
            platform_sounds='Default IM Mail Reminder SMS'
            ;;
        *)
            platform_sounds='bell complete message message-new-instant dialog-information dialog-warning dialog-error trash-empty'
            ;;
    esac
    COMPREPLY=( $(compgen -W "zetta-default zetta-ok zetta-alarm $platform_sounds" -- "$current") )
}

_ztftp_complete() {
    _zetta_tftp_complete 1
}

_zntfy_complete() {
    local current previous
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}

    case "$previous" in
        --app-name|-a)
            COMPREPLY=()
            return
            ;;
        --icon|-i)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        --sound|-s)
            _zetta_complete_sound_names
            return
            ;;
        --timeout|-t)
            COMPREPLY=( $(compgen -W 'default never' -- "$current") )
            return
            ;;
    esac
    _zetta_compgen '--app-name --icon --sound --timeout --help'
}

_zcopy_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
    esac
    _zetta_compgen '--pboard --help'
}

_zpaste_complete() {
    local current=${COMP_WORDS[COMP_CWORD]} previous=${COMP_WORDS[COMP_CWORD-1]}
    case "$previous" in
        --pboard|-pboard)
            _zetta_compgen 'general ruler find font'
            return
            ;;
        --prefer|-prefer|--Prefer|-Prefer)
            _zetta_compgen 'txt rtf ps'
            return
            ;;
    esac
    _zetta_compgen '--pboard --prefer --help'
}

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }
zcopy() { zetta copy "$@"; }
zpaste() { zetta paste "$@"; }
complete -F _zetta_complete zetta
complete -F _zetta_complete zvi
complete -F _ztftp_complete ztftp
complete -F _zntfy_complete zntfy
complete -F _zcopy_complete zcopy
complete -F _zpaste_complete zpaste

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone there.
# Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working; any
# preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is removed
# first so Zetta's functions take priority over it.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        pbcopy() { zetta copy "$@"; }
        pbpaste() { zetta paste "$@"; }
        complete -F _zcopy_complete pbcopy
        complete -F _zpaste_complete pbpaste
        ;;
esac
