# Zetta shell integration for Bash.
if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]]; then
    zetta() { command "$ZETTA_HOST_EXECUTABLE" "$@"; }
fi

if [[ -z ${__ZETTA_CWD_TRACKING_INSTALLED:-} ]]; then
    __ZETTA_CWD_TRACKING_INSTALLED=1
    __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    if [[ -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} ]]; then
        __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
    else
        __ZETTA_LIFECYCLE_TRACKING_ENABLED=0
    fi
    __ZETTA_COMMAND_STARTED=0
    __zetta_at_prompt=0
    __zetta_report_tracking_ready() {
        [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 ]] || return
        printf '\033]2;zetta-event:tracking-ready\033\\'
    }
    __zetta_report_command_started() {
        [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 ]] || return
        [[ ${__zetta_at_prompt:-0} == 1 ]] || return
        __zetta_at_prompt=0
        case "$BASH_COMMAND" in
            __zetta_report_cwd|__zetta_mark_prompt) return ;;
        esac
        __ZETTA_COMMAND_STARTED=1
        printf '\033]2;zetta-event:command-started:%s\033\\' "$BASH_COMMAND"
    }
    __zetta_report_cwd() {
        local status=$?
        if [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 && ${__ZETTA_COMMAND_STARTED:-0} == 1 ]]; then
            printf '\033]2;zetta-event:command-finished:%s\033\\' "$status"
            __ZETTA_COMMAND_STARTED=0
        fi
        printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
        return "$status"
    }
    __zetta_mark_prompt() {
        __zetta_at_prompt=1
    }
    if [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 ]]; then
        if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
            PROMPT_COMMAND=(__zetta_report_cwd "${PROMPT_COMMAND[@]}" __zetta_mark_prompt)
        else
            PROMPT_COMMAND="__zetta_report_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__zetta_mark_prompt"
        fi
        trap '__zetta_report_command_started' DEBUG
    elif [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
        PROMPT_COMMAND+=(__zetta_report_cwd)
    else
        PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__zetta_report_cwd"
    fi
    __zetta_report_tracking_ready
fi

# Upgrade a shell that loaded an older CWD-only integration. Keep this
# separate from the CWD guard: the shell may already have
# __ZETTA_CWD_TRACKING_INSTALLED set when a new Zetta binary is installed.
if [[ -z ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-} ||
    ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} &&
        ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then
    __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    if [[ -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} ]]; then
        __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
        __ZETTA_COMMAND_STARTED=0
        __zetta_at_prompt=0
        __zetta_report_tracking_ready() {
            [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 ]] || return
            printf '\033]2;zetta-event:tracking-ready\033\\'
        }
        __zetta_report_command_started() {
            [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 ]] || return
            [[ ${__zetta_at_prompt:-0} == 1 ]] || return
            __zetta_at_prompt=0
            case "$BASH_COMMAND" in
                __zetta_report_cwd|__zetta_mark_prompt) return ;;
            esac
            __ZETTA_COMMAND_STARTED=1
            printf '\033]2;zetta-event:command-started:%s\033\\' "$BASH_COMMAND"
        }
        # An older integration already registered this function in
        # PROMPT_COMMAND. Redefining it upgrades that registration in place.
        __zetta_report_cwd() {
            local status=$?
            if [[ ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} == 1 && ${__ZETTA_COMMAND_STARTED:-0} == 1 ]]; then
                printf '\033]2;zetta-event:command-finished:%s\033\\' "$status"
                __ZETTA_COMMAND_STARTED=0
            fi
            printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
            return "$status"
        }
        __zetta_mark_prompt() {
            __zetta_at_prompt=1
        }
        if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
            PROMPT_COMMAND+=(__zetta_mark_prompt)
        else
            PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__zetta_mark_prompt"
        fi
        trap '__zetta_report_command_started' DEBUG
        __zetta_report_tracking_ready
    else
        __ZETTA_LIFECYCLE_TRACKING_ENABLED=0
    fi
fi

if [[ -z ${EDITOR+x} ]]; then
    export EDITOR='zetta vi'
fi

if ! type -t vi >/dev/null 2>&1; then
    eval 'vi() { zetta vi "$@"; }'
    complete -F _zetta_complete vi
fi

zvi() { zetta vi "$@"; }

# ZETTA_WORKTREE_INTEGRATION_BEGIN
zwt() {
    case $1 in
        new)
            local path path_only_arg
            local -a operation_args=("${@:2}")
            for path_only_arg in "${operation_args[@]}"; do
                if [[ $path_only_arg == --help || $path_only_arg == -h ]]; then
                    command zwt new "${operation_args[@]}"
                    return
                fi
                if [[ $path_only_arg == --path-only || $path_only_arg == -P ]]; then
                    path_only_arg=1
                    break
                fi
                path_only_arg=''
            done
            if [[ $path_only_arg == 1 ]]; then
                path=$(command zwt new "${operation_args[@]}") || return
            else
                path=$(command zwt new --path-only "${operation_args[@]}") || return
            fi
            [[ -n $path ]] || return 1
            builtin cd -- "$path"
            ;;
        done)
            local path path_only_arg
            local -a operation_args=("${@:2}")
            for path_only_arg in "${operation_args[@]}"; do
                if [[ $path_only_arg == --help || $path_only_arg == -h ]]; then
                    command zwt done "${operation_args[@]}"
                    return
                fi
                if [[ $path_only_arg == --path-only || $path_only_arg == -P ]]; then
                    path_only_arg=1
                    break
                fi
                path_only_arg=''
            done
            if [[ $path_only_arg == 1 ]]; then
                path=$(command zwt done "${operation_args[@]}") || return
            else
                path=$(command zwt done --path-only "${operation_args[@]}") || return
            fi
            [[ -n $path ]] || return 1
            builtin cd -- "$path"
            ;;
        abort)
            local path path_only_arg
            local -a operation_args=("${@:2}")
            for path_only_arg in "${operation_args[@]}"; do
                if [[ $path_only_arg == --help || $path_only_arg == -h ]]; then
                    command zwt abort "${operation_args[@]}"
                    return
                fi
                if [[ $path_only_arg == --path-only || $path_only_arg == -P ]]; then
                    path_only_arg=1
                else
                    path_only_arg=''
                fi
            done
            if [[ $path_only_arg == 1 ]]; then
                path=$(command zwt abort "${operation_args[@]}") || return
            else
                path=$(command zwt abort --path-only "${operation_args[@]}") || return
            fi
            [[ -n $path ]] || return 1
            builtin cd -- "$path"
            ;;
        *)
            command zwt "$@"
            ;;
    esac
}
# ZETTA_WORKTREE_INTEGRATION_END

_zetta_option_used() {
    local option=$1 index
    for (( index = 1; index < COMP_CWORD; index++ )); do
        [[ ${COMP_WORDS[index]} == "$option" ]] && return 0
    done
    return 1
}

_zetta_compgen() {
    local options=$1 repeatable=${2:-0} candidate
    local -a available=()
    for candidate in $options; do
        if [[ $candidate != -* ]] || ! _zetta_option_used "$candidate" || ZETTA_WORKTREE_BASH_REPEATABLE_COPY; then
            available+=("$candidate")
        fi
    done
    COMPREPLY=( $(compgen -W "${available[*]}" -- "$current") )
}

_zetta_complete() {
    local current previous command pane_operation profile_operation profile_command_index=-1 command_option_index=-1
    local -a config_args=()
    current=${COMP_WORDS[COMP_CWORD]}
    previous=${COMP_WORDS[COMP_CWORD-1]}
    command=${COMP_WORDS[1]}
    pane_operation=''
    if [[ $command == pane && ${COMP_WORDS[2]} == wait ]]; then
        pane_operation=wait
    fi

    local index
    for (( index = 1; index < COMP_CWORD; index++ )); do
        if [[ ${COMP_WORDS[index]} == --config || ${COMP_WORDS[index]} == -c ]]; then
            if (( index + 1 < COMP_CWORD )); then
                config_args+=(--config "${COMP_WORDS[index+1]}")
                (( index++ ))
            fi
        elif [[ ${COMP_WORDS[index]} == --command || ${COMP_WORDS[index]} == -e ]]; then
            break
        fi
    done

    profile_operation=''
    for (( index = 1; index < COMP_CWORD; index++ )); do
        case ${COMP_WORDS[index]} in
            --config|-c|--keymap|-k|--profile|-p|--split|-s|--theme|-t)
                (( index++ ))
                ;;
            --command|-e)
                command_option_index=$index
                break
                ;;
            profile)
                profile_command_index=$index
                command=profile
                break
                ;;
        esac
    done
    if (( profile_command_index >= 0 )); then
        index=$((profile_command_index + 1))
        while (( index < COMP_CWORD )); do
            case ${COMP_WORDS[index]} in
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
                    profile_operation=${COMP_WORDS[index]}
                    break
                    ;;
            esac
        done
    fi
    if (( command_option_index >= 0 )); then
        COMPREPLY=()
        return
    fi

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
        while IFS= read -r profile; do
            [[ $profile == "$current"* ]] && COMPREPLY+=("$profile")
        done < <(zetta profile list "${config_args[@]}" 2>/dev/null)
    }

    _zetta_complete_profile_themes() {
        COMPREPLY=()
        local theme
        while IFS= read -r theme; do
            [[ $theme == "$current"* ]] && COMPREPLY+=("$theme")
        done < <(zetta profile themes "${config_args[@]}" 2>/dev/null)
    }

    _zetta_complete_projects() {
        COMPREPLY=()
        local project
        while IFS= read -r project; do
            [[ $project == "$current"* ]] && COMPREPLY+=("$project")
        done < <(zetta project list 2>/dev/null)
    }

    _zetta_complete_project_commands() {
        COMPREPLY=()
        local command_name
        while IFS= read -r command_name; do
            [[ $command_name == "$current"* ]] && COMPREPLY+=("$command_name")
        done < <(zetta cmd --list 2>/dev/null)
    }

    _zetta_complete_project_command_options() {
        local option
        for option in "$@"; do
            [[ $option == "$current"* ]] || continue
            _zetta_option_used "$option" || COMPREPLY+=("$option")
        done
    }

    _zetta_complete_mux_session_ids() {
        COMPREPLY=()
        local session_id
        local -a mux_list_command=(zetta mux list)
        if [[ ${_zetta_mux_completion_command:-} == zmux ]]; then
            mux_list_command=(zmux list)
        fi
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <("${mux_list_command[@]}" 2>/dev/null | awk '$1 == "reconnect" && $2 == "id:" && $3 ~ /^[0-9]+:[0-9]+:[0-9]+$/ { print $3 }')
    }

    _zetta_complete_mux_restorable_ids() {
        COMPREPLY=()
        local session_id
        local -a mux_list_command=(zetta mux list)
        if [[ ${_zetta_mux_completion_command:-} == zmux ]]; then
            mux_list_command=(zmux list)
        fi
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <("${mux_list_command[@]}" 2>/dev/null | awk '$1 == "resume" && $2 == "id:" && $3 ~ /^[0-9]+$/ { print $3 }')
    }

    _zetta_complete_tab_icons() {
        local icons
        icons=$(zetta tabicon --list 2>/dev/null)
        COMPREPLY=( $(compgen -W "$icons" -- "$current") )
    }

    _zetta_complete_themes() {
        local scope=$1
        COMPREPLY=()
        local theme
        while IFS= read -r theme; do
            [[ $theme == "$current"* ]] && COMPREPLY+=("$theme")
        done < <(zetta theme "$scope" --list 2>/dev/null)
    }

    _zetta_complete_pane_splits() {
        COMPREPLY=()
        local split
        while IFS= read -r split; do
            [[ $split == "$current"* ]] && COMPREPLY+=("$split")
        done < <(zetta splits 2>/dev/null)
    }

    _zetta_complete_pane_labels() {
        COMPREPLY=()
        local label
        while IFS= read -r label; do
            [[ $label == "$current"* ]] && COMPREPLY+=("$label")
        done < <(zetta pane --list 2>/dev/null)
    }

    _zetta_complete_run_pane_labels() {
        COMPREPLY=()
        local prefix='' partial="$current" label selected_label duplicate
        local -a selected=()
        if [[ $current == *,* ]]; then
            prefix="${current%,*},"
            partial=${current##*,}
            IFS=',' read -r -a selected <<< "${current%,*}"
        fi
        while IFS= read -r label; do
            [[ $label == "$partial"* ]] || continue
            duplicate=0
            for selected_label in "${selected[@]}"; do
                if [[ $selected_label == "$label" ]]; then
                    duplicate=1
                    break
                fi
            done
            (( duplicate )) || COMPREPLY+=("$prefix$label")
        done < <(zetta pane --list 2>/dev/null)
    }

    if [[ $pane_operation == wait ]]; then
        local wait_delimiter=0 wait_dependency=0 argument
        for (( index = 3; index < COMP_CWORD; index++ )); do
            argument=${COMP_WORDS[index]}
            if [[ $argument == -- ]]; then
                wait_delimiter=1
                break
            elif [[ $argument != --allow-failure && $argument != -a ]]; then
                wait_dependency=1
            fi
        done
        if (( wait_delimiter )); then
            COMPREPLY=()
        elif [[ $current == -* ]]; then
            _zetta_compgen '--allow-failure --help --'
        elif [[ $previous == wait || $previous == --allow-failure || $previous == -a || $current == *,* || $wait_dependency == 0 ]]; then
            _zetta_complete_run_pane_labels
        else
            _zetta_compgen '--allow-failure --help --'
        fi
        return
    fi

    if [[ $command == cmd ]]; then
        local cmd_delimiter=0
        for (( index = 2; index < COMP_CWORD; index++ )); do
            if [[ ${COMP_WORDS[index]} == -- ]]; then
                cmd_delimiter=1
                break
            fi
        done
        if (( cmd_delimiter )); then
            COMPREPLY=()
        elif (( COMP_CWORD == 2 )); then
            if [[ $current == -* ]]; then
                _zetta_compgen '--help --list --'
            else
                _zetta_complete_project_commands
                _zetta_complete_project_command_options '--help' '--list' '--'
            fi
        elif [[ ${COMP_WORDS[2]} == --list || ${COMP_WORDS[2]} == --help ]]; then
            COMPREPLY=()
        elif (( COMP_CWORD == 3 )); then
            if [[ $current == -* ]]; then
                _zetta_compgen '--help --'
            else
                COMPREPLY=()
                _zetta_complete_project_command_options '--help' '--'
            fi
        elif [[ $current == -* ]]; then
            _zetta_compgen '--help --'
        else
            COMPREPLY=()
        fi
        return
    fi

    case "$previous" in
        --command|-e)
            COMPREPLY=()
            return
            ;;
        --)
            COMPREPLY=()
            return
            ;;
# ZETTA_WORKTREE_INTEGRATION_BEGIN
        --copy)
            if [[ $command == wt && ${COMP_WORDS[2]} == new ]]; then
                COMPREPLY=( $(compgen -f -- "$current") )
            else
                COMPREPLY=()
            fi
            return
            ;;
# ZETTA_WORKTREE_INTEGRATION_END
        --profile)
            _zetta_complete_profiles
            return
            ;;
        --pane)
            if [[ $command == pane ]]; then
                _zetta_complete_pane_labels
            else
                COMPREPLY=()
            fi
            return
            ;;
        --direction)
            if [[ $command == pane ]]; then
                _zetta_compgen 'left right up down'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --overlay-size|-S)
            if [[ $command == pane ]]; then
                _zetta_compgen 'sm base lg xl 2xl 3xl'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --overlay-opacity|-O|--overlay)
            COMPREPLY=()
            return
            ;;
        -p)
            if [[ $command == pane ]]; then
                _zetta_complete_pane_labels
            elif [[ $command == profile && $profile_operation == add ]]; then
                COMPREPLY=()
            elif [[ $command == serial ]]; then
                _zetta_compgen 'none odd even'
            elif [[ $command != tftp && $command != http && $command != notify && $command != attention ]]; then
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
            if [[ $command == pane ]]; then
                _zetta_compgen 'left right up down'
            elif [[ $command == serial ]]; then
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
        --split)
            _zetta_complete_pane_splits
            return
            ;;
        --replace-pane)
            if [[ $current == -* || -z $current ]]; then
                _zetta_compgen '--help --version --config --keymap --profile --split --theme --no-mux --new-window --command'
            else
                COMPREPLY=()
            fi
            return
            ;;
        --stop-bits|--size)
            if [[ $command == serial ]]; then
                _zetta_compgen '1 2'
            elif [[ $command == notify || $command == attention ]]; then
                _zetta_complete_sound_names
            elif [[ $command == overlay ]]; then
                _zetta_compgen 'sm base lg xl 2xl 3xl'
            else
                COMPREPLY=()
            fi
            return
            ;;
        -s)
            if [[ $command == -* || -z $command ]]; then
                _zetta_complete_pane_splits
            elif [[ $command == serial ]]; then
                _zetta_compgen '1 2'
            elif [[ $command == notify || $command == attention ]]; then
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
            elif [[ $command == profile && ($profile_operation == add || $profile_operation == icon) ]]; then
                _zetta_compgen 'auto zetta bash zsh fish'
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
        --config|--keymap|-k|--profile-report|--program|--arg|-a)
            COMPREPLY=( $(compgen -f -- "$current") )
            return
            ;;
        -c)
            if ZETTA_WORKTREE_BASH_COPY_CONDITION; then
                COMPREPLY=( $(compgen -f -- "$current") )
            elif [[ $command == overlay ]]; then
                _zetta_compgen 'ZETTA_OVERLAY_COLORS'
            elif [[ $command == pane ]]; then
                _zetta_compgen 'ZETTA_OVERLAY_COLORS'
            elif [[ $command == terminal-size ]]; then
                COMPREPLY=()
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --color|--overlay-color)
            if [[ $command == overlay || $command == pane ]]; then
                _zetta_compgen 'ZETTA_OVERLAY_COLORS'
            else
                COMPREPLY=()
            fi
            return
            ;;
        -r)
            if [[ $command == http || ( $command == tftp && ${COMP_WORDS[2]} == server ) ]]; then
                COMPREPLY=( $(compgen -d -- "$current") )
            elif [[ $command == terminal-size || $command == profile ]]; then
                COMPREPLY=()
            elif [[ $command == -* || -z $command ]]; then
                if [[ $current == -* || -z $current ]]; then
                    _zetta_compgen '--help --version --config --keymap --profile --split --theme --no-mux --new-window --command'
                else
                    COMPREPLY=()
                fi
            else
                COMPREPLY=( $(compgen -f -- "$current") )
            fi
            return
            ;;
        --output-type|-t|--theme|--text)
            if [[ $command == profile ]]; then
                _zetta_complete_profile_themes
            elif [[ $command == -* ]]; then
                _zetta_complete_profile_themes
            elif [[ $command == theme && ${COMP_WORDS[2]} == pane ]]; then
                _zetta_complete_themes pane
            elif [[ $command == theme && ${COMP_WORDS[2]} == tab ]]; then
                _zetta_complete_themes tab
            elif [[ $command == notify || $command == attention ]]; then
                _zetta_compgen 'default never'
            elif [[ $command == overlay ]]; then
                COMPREPLY=()
            elif [[ $command == benchmark && ${COMP_WORDS[2]} == output ]]; then
                _zetta_compgen 'repeated unique'
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
        _zetta_compgen 'benchmark terminal-size mux pane profile project cmd edit vi init serial http tftp notify attention copy paste splits tabicon theme overlay ZETTA_WORKTREE_ROOT_COMMAND --help --version --config --keymap --profile --split --replace-pane --theme --no-mux --new-window --command'
        return
    fi

    # A leading flag rules out a subcommand for the rest of the command line
    # (subcommands are only recognized as the first argument), so keep
    # offering the remaining top-level flags instead of falling through to
    # the subcommand-specific cases below, which would offer nothing.
    if [[ $command == -* ]]; then
        _zetta_compgen '--help --version --config --keymap --profile --split --replace-pane --theme --no-mux --new-window --command'
        return
    fi

    case "$command" in
        profile)
            if (( profile_command_index >= 0 && COMP_CWORD == profile_command_index + 1 )); then
                _zetta_compgen 'list themes disable enable theme dark-theme icon default add remove --help'
            elif [[ $current == -* ]]; then
                case "$profile_operation" in
                    theme|dark-theme) _zetta_compgen '--reset --config --help' ;;
                    icon) _zetta_compgen '--reset --config --help' ;;
                    add) _zetta_compgen '--program --arg --theme --dark-theme --icon --config --help' ;;
                    *) _zetta_compgen '--config --help' ;;
                esac
            else
                case "$profile_operation" in
                    disable|enable|default|remove)
                        if [[ $previous == "$profile_operation" ]]; then
                            _zetta_complete_profiles
                        else
                            _zetta_compgen '--config --help'
                        fi
                        ;;
                    theme|dark-theme)
                        if [[ $current == -* ]]; then
                            _zetta_compgen '--reset --config --help'
                        else
                            local positional=0 skip=0 argument
                            for (( index = profile_command_index + 2; index < COMP_CWORD; index++ )); do
                                argument=${COMP_WORDS[index]}
                                if (( skip )); then
                                    skip=0
                                elif [[ $argument == --config || $argument == -c ]]; then
                                    skip=1
                                elif [[ $argument == --reset || $argument == -r ]]; then
                                    :
                                elif [[ $argument != -* ]]; then
                                    (( positional++ ))
                                fi
                            done
                            if (( positional == 0 )); then
                                _zetta_complete_profiles
                            else
                                _zetta_complete_profile_themes
                            fi
                        fi
                        ;;
                    icon)
                        if [[ $previous == icon ]]; then
                            _zetta_complete_profiles
                        elif [[ $previous == --reset || $previous == -r ]]; then
                            _zetta_compgen '--config --help'
                        else
                            _zetta_compgen 'auto zetta bash zsh fish'
                        fi
                        ;;
                    add)
                        if [[ $previous == --theme || $previous == -t || $previous == --dark-theme || $previous == -d ]]; then
                            _zetta_complete_profile_themes
                        else
                            _zetta_compgen '--program --arg --theme --dark-theme --icon --config --help'
                        fi
                        ;;
                    *)
                        _zetta_compgen '--config --help'
                        ;;
                esac
            fi
            ;;
        project)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'add list remove open --help'
            else
                case ${COMP_WORDS[2]} in
                    add)
                        if [[ $current == -* ]]; then
                            _zetta_compgen '--path --help'
                        else
                            COMPREPLY=( $(compgen -d -- "$current") )
                        fi
                        ;;
                    open|remove)
                        if [[ $current == -* ]]; then
                            _zetta_compgen '--path --help'
                        else
                            _zetta_complete_projects
                        fi
                        ;;
                    list) _zetta_compgen '--help' ;;
                esac
            fi
            ;;
        cmd)
            if (( COMP_CWORD == 2 )); then
                if [[ $current == -* ]]; then
                    _zetta_compgen '--help --list --'
                else
                    _zetta_complete_project_commands
                    _zetta_complete_project_command_options '--help' '--list' '--'
                fi
            elif [[ ${COMP_WORDS[2]} == --list || ${COMP_WORDS[2]} == --help ]]; then
                COMPREPLY=()
            elif (( COMP_CWORD == 3 )); then
                if [[ $current == -* ]]; then
                    _zetta_compgen '--help --'
                else
                    COMPREPLY=()
                    _zetta_complete_project_command_options '--help' '--'
                fi
            else
                COMPREPLY=()
            fi
            ;;
        benchmark)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'output --profile-report --profile-duration --profile-pane-stress --profile-background-stress --profile-sparse-updates --profile-alt-screen-scroll --profile-external-terminal --help'
            elif [[ ${COMP_WORDS[2]} == output ]]; then
                _zetta_compgen '--size --output-type --help'
            else
                _zetta_compgen '--profile-report --profile-duration --profile-pane-stress --profile-background-stress --profile-sparse-updates --profile-alt-screen-scroll --profile-external-terminal --help'
            fi
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
        mux)
            if (( COMP_CWORD == 2 )); then
                if [[ ${ZETTA_NO_MUX:-0} == 1 ]]; then
                    _zetta_compgen 'list reconnect --json --help --version'
                else
                    _zetta_compgen 'list stop reconnect resume share unshare kill forget --json --upgrade --help --version'
                fi
            elif [[ ${ZETTA_NO_MUX:-0} == 1 && ${COMP_WORDS[2]} != reconnect && ${COMP_WORDS[2]} != list ]]; then
                COMPREPLY=()
            elif [[ ${COMP_WORDS[2]} == stop ]]; then
                _zetta_compgen '--force --help'
            elif [[ ${COMP_WORDS[2]} == reconnect ]] && (( COMP_CWORD == 3 )); then
                _zetta_complete_mux_session_ids
            elif [[ ${COMP_WORDS[2]} == resume && $current != -* ]] && (( COMP_CWORD == 3 )); then
                _zetta_complete_mux_restorable_ids
            elif [[ ${ZETTA_NO_MUX:-0} != 1 && ( ${COMP_WORDS[2]} == share || ${COMP_WORDS[2]} == unshare || ${COMP_WORDS[2]} == kill || ${COMP_WORDS[2]} == forget ) ]] && (( COMP_CWORD == 3 )); then
                _zetta_complete_mux_session_ids
            elif [[ ( ${COMP_WORDS[2]} == resume || ${COMP_WORDS[2]} == reconnect ) && ${COMP_WORDS[COMP_CWORD-1]} == --identity ]]; then
                COMPREPLY=( $(compgen -f -- "$current") )
            elif [[ ${COMP_WORDS[2]} == resume || ${COMP_WORDS[2]} == reconnect ]]; then
                _zetta_compgen '--identity --help'
            else
                _zetta_compgen '--json --identity --help'
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
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'cleanup --app-name --icon --sound --timeout --help'
            elif [[ ${COMP_WORDS[2]} == cleanup ]]; then
                _zetta_compgen '--dry-run --help'
            else
                _zetta_compgen '--app-name --icon --sound --timeout --help'
            fi
            ;;
        attention)
            _zetta_compgen '--notify --app-name --icon --sound --timeout --help'
            ;;
        copy)
            _zetta_compgen '--pboard --help'
            ;;
        paste)
            _zetta_compgen '--pboard --prefer --help'
            ;;
        splits)
            _zetta_compgen '--help'
            ;;
        pane)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'wait --direction --label --pane --overlay --overlay-size --overlay-opacity --overlay-color --stack --list --help'
            else
                _zetta_compgen '--direction --label --pane --overlay --overlay-size --overlay-opacity --overlay-color --stack --list --help'
            fi
            ;;
        tabicon)
            if [[ $current == -* ]]; then
                _zetta_compgen '--icon --list --help'
            else
                _zetta_complete_tab_icons
            fi
            ;;
        theme)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'pane tab'
            elif [[ ${COMP_WORDS[2]} == pane || ${COMP_WORDS[2]} == tab ]]; then
                if [[ $current == -* ]]; then
                    _zetta_compgen '--theme --reset --list --help'
                else
                    _zetta_complete_themes "${COMP_WORDS[2]}"
                fi
            else
                COMPREPLY=()
            fi
            ;;
        overlay)
            _zetta_compgen '--text --size --opacity --color --reset --help'
            ;;
# ZETTA_WORKTREE_INTEGRATION_BEGIN
        wt)
            if (( COMP_CWORD == 2 )); then
                _zetta_compgen 'new done abort status sync config --help'
            elif [[ ${COMP_WORDS[2]} == new || ${COMP_WORDS[2]} == done || ${COMP_WORDS[2]} == abort ]]; then
                if [[ ${COMP_WORDS[2]} == new ]]; then
                    _zetta_compgen '--copy --path-only --help' 1
                else
                    _zetta_compgen '--path-only --help'
                fi
            elif [[ ${COMP_WORDS[2]} == sync ]]; then
                if (( COMP_CWORD == 3 )); then
                    if [[ $current == -* ]]; then
                        _zetta_compgen '--help'
                    else
                        _zetta_complete_worktree_commits
                    fi
                elif [[ $current == -* ]]; then
                    _zetta_compgen '--help'
                else
                    COMPREPLY=()
                fi
            else
                _zetta_compgen '--help'
            fi
            ;;
# ZETTA_WORKTREE_INTEGRATION_END
    esac
}

# ZETTA_WORKTREE_INTEGRATION_BEGIN
_zetta_complete_worktree_commits() {
    COMPREPLY=()
    local current_branch source_branch split_point commit
    current_branch=$(git branch --show-current 2>/dev/null) || return
    [[ -n $current_branch ]] || return
    source_branch=$(git config --local --get "wtbranch.${current_branch}.base" 2>/dev/null) || return
    [[ -n $source_branch ]] || return
    split_point=$(git merge-base "refs/heads/${current_branch}" "refs/heads/${source_branch}" 2>/dev/null) || return
    [[ -n $split_point ]] || return
    while IFS= read -r commit; do
        [[ $commit == "$current"* ]] && COMPREPLY+=("$commit")
    done < <(git rev-list --reverse "${split_point}..refs/heads/${source_branch}" 2>/dev/null)
}

_zetta_complete_zwt() {
    local saved_words=("${COMP_WORDS[@]}")
    local saved_cword=$COMP_CWORD
    COMP_WORDS=(zetta wt "${COMP_WORDS[@]:1}")
    (( COMP_CWORD++ ))
    _zetta_complete
    COMP_WORDS=("${saved_words[@]}")
    COMP_CWORD=$saved_cword
}
# ZETTA_WORKTREE_INTEGRATION_END

_zetta_complete_zmux() {
    local _zetta_mux_completion_command=zmux
    local saved_words=("${COMP_WORDS[@]}")
    local saved_cword=$COMP_CWORD
    COMP_WORDS=(zetta mux "${COMP_WORDS[@]:1}")
    (( COMP_CWORD++ ))
    _zetta_complete
    COMP_WORDS=("${saved_words[@]}")
    COMP_CWORD=$saved_cword
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
            _zetta_compgen '--root --port --config --writable --help'
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

# zetta-default/zetta-ok/zetta-alarm/zetta-gong are bundled tones Zetta plays itself, so
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
    COMPREPLY=( $(compgen -W "zetta-default zetta-ok zetta-alarm zetta-gong $platform_sounds" -- "$current") )
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
# ZETTA_WORKTREE_INTEGRATION_BEGIN
complete -F _zetta_complete_zwt zwt
# ZETTA_WORKTREE_INTEGRATION_END
complete -F _zetta_complete_zmux zmux
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
