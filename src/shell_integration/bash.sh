# Zetta shell integration for Bash.
__zetta_run_path() {
    command zetta "$@"
}

__zetta_run_owner() {
    if [[ -n ${ZETTA_HOST_EXECUTABLE:-} && -f "$ZETTA_HOST_EXECUTABLE" && -x "$ZETTA_HOST_EXECUTABLE" ]]; then
        command "$ZETTA_HOST_EXECUTABLE" "$@"
    else
        command zetta "$@"
    fi
}

__zetta_profile_uses_owner() {
    local -a profile_arguments=("$@")
    local index=0 argument
    while (( index < ${#profile_arguments[@]} )); do
        argument=${profile_arguments[index]}
        case $argument in
            --config|-c)
                (( index += 2 ))
                ;;
            profile)
                (( index++ ))
                while (( index < ${#profile_arguments[@]} )); do
                    argument=${profile_arguments[index]}
                    case $argument in
                        --config|-c)
                            (( index += 2 ))
                            ;;
                        --help|-h|list|themes)
                            return 1
                            ;;
                        *)
                            return 0
                            ;;
                    esac
                done
                return 1
                ;;
            *)
                return 1
                ;;
        esac
    done
    return 1
}

__zetta_should_use_owner() {
    if (( $# == 0 )); then
        return 0
    fi

    case $1 in
        pane|attention|notify|theme|overlay)
            return 0
            ;;
        cmd)
            [[ $# -ge 2 ]] || return 1
            case $2 in
                --list|-l|--help|-h)
                    return 1
                    ;;
            esac
            local index argument
            for (( index = 3; index <= $#; index++ )); do
                argument=${!index}
                case $argument in
                    --)
                        return 0
                        ;;
                    --help|-h)
                        return 1
                        ;;
                esac
            done
            return 0
            ;;
        project)
            case ${2:-} in
                add|remove|open)
                    return 0
                    ;;
            esac
            return 1
            ;;
        tabicon)
            case ${2:-} in
                --list|-l|--help|-h|'')
                    return 1
                    ;;
                *)
                    return 0
                    ;;
            esac
            ;;
        profile)
            __zetta_profile_uses_owner "$@"
            return $?
            ;;
    esac

    local -a arguments=("$@")
    local index=0 argument
    while (( index < ${#arguments[@]} )); do
        argument=${arguments[index]}
        case $argument in
            --config|-c|--keymap|-k|--profile|-p|--split|-s|--theme|-t|--geometry|-g|--zetta-profile-actions-generation)
                (( index += 2 ))
                ;;
            --new-window|-w|--command|-e|--replace-pane|-r)
                return 0
                ;;
            profile)
                __zetta_profile_uses_owner "$@"
                return $?
                ;;
            --)
                return 1
                ;;
            *)
                return 1
                ;;
        esac
    done
    return 1
}

if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]]; then
    zetta() {
        if __zetta_should_use_owner "$@"; then
            __zetta_run_owner "$@"
        else
            __zetta_run_path "$@"
        fi
    }
fi

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
# Define the tracker once; both initial setup and upgrades use it.
if [[ -z ${__ZETTA_CWD_TRACKING_INSTALLED:-} ]]; then
    __ZETTA_CWD_TRACKING_INSTALLED=1
    if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
        PROMPT_COMMAND=(__zetta_report_cwd "${PROMPT_COMMAND[@]}")
    else
        PROMPT_COMMAND="__zetta_report_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
    fi
fi
if [[ -z ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-} ||
    ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} &&
        ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then
    __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    __ZETTA_LIFECYCLE_TRACKING_ENABLED=0
    if [[ -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} ]]; then
        __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
        __ZETTA_COMMAND_STARTED=0
        __zetta_at_prompt=0
        if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
            PROMPT_COMMAND+=(__zetta_mark_prompt)
        else
            PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__zetta_mark_prompt"
        fi
        trap '__zetta_report_command_started' DEBUG
        __zetta_report_tracking_ready
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

if ! type -t mosh >/dev/null 2>&1; then
    mosh() { zetta mosh "$@"; }
    __ZETTA_MOSH_WRAPPER=1
fi

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

# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
unset -f zcopy zpaste 2>/dev/null
# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone there.
# Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working; any
# preexisting pbcopy/pbpaste alias (eg. one pointing at xclip) is removed
# first so Zetta's functions take priority over it.
case "$OSTYPE" in
    darwin*) ;;
    *)
        unalias pbcopy pbpaste 2>/dev/null
        pbcopy() { command zcopy "$@"; }
        pbpaste() { command zpaste "$@"; }
        ;;
esac
# ZETTA_CLIPBOARD_INTEGRATION_END

ztftp() { zetta tftp "$@"; }

zntfy() { zetta notify "$@"; }

# Completion code is generated once, on the first request. Failed loads retry.
__zetta_lazy_complete() {
    [[ ${__ZETTA_COMPLETIONS_LOADING:-0} == 0 ]] || return
    if [[ ${__ZETTA_COMPLETIONS_LOADED:-0} != 1 ]]; then
        local payload
        __ZETTA_COMPLETIONS_LOADING=1
        if payload=$(zetta init bash --completions) && [[ -n $payload ]] && builtin eval "$payload"; then
            __ZETTA_COMPLETIONS_LOADED=1
        fi
        __ZETTA_COMPLETIONS_LOADING=0
        [[ ${__ZETTA_COMPLETIONS_LOADED:-0} == 1 ]] || return
    fi
    local handler
    handler=$(complete -p -- "${COMP_WORDS[0]}" 2>/dev/null)
    handler=${handler#* -F }
    handler=${handler%% *}
    [[ $handler != __zetta_lazy_complete && -n $handler ]] && "$handler" "$@"
}
if [[ ${__ZETTA_COMPLETIONS_LOADED:-0} != 1 ]]; then
    complete -F __zetta_lazy_complete zetta zosh zvi ztftp zntfy
    [[ ${__ZETTA_MOSH_WRAPPER:-0} == 1 ]] && complete -F __zetta_lazy_complete mosh
# ZETTA_WORKTREE_INTEGRATION_BEGIN
    complete -F __zetta_lazy_complete zwt
# ZETTA_WORKTREE_INTEGRATION_END
# ZETTA_ZMUX_INTEGRATION_BEGIN
    complete -F __zetta_lazy_complete zmux
# ZETTA_ZMUX_INTEGRATION_END
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
    complete -F __zetta_lazy_complete zcopy zpaste
    case "$OSTYPE" in darwin*) ;; *) complete -F __zetta_lazy_complete pbcopy pbpaste ;; esac
# ZETTA_CLIPBOARD_INTEGRATION_END
fi
