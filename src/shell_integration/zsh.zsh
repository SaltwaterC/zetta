# Zetta shell integration for Zsh.
if (( ! $+functions[__zetta_filter_startup_history] )); then
    function __zetta_filter_startup_history() {
        if [[ "$1" == *"__zed_init_command_history_"* ]]; then
            fc -p
            return 1
        fi
        return 0
    }
fi
autoload -Uz add-zsh-hook
(( ${zshaddhistory_functions[(I)__zetta_filter_startup_history]:-0} == 0 )) &&
    add-zsh-hook zshaddhistory __zetta_filter_startup_history

function __zetta_run_path { command zetta "$@"; }

function __zetta_run_owner {
    if [[ -n ${ZETTA_HOST_EXECUTABLE:-} && -f "$ZETTA_HOST_EXECUTABLE" && -x "$ZETTA_HOST_EXECUTABLE" ]]; then
        command "$ZETTA_HOST_EXECUTABLE" "$@"
    else
        command zetta "$@"
    fi
}

function __zetta_profile_uses_owner {
    local index=1 argument
    while (( index <= $# )); do
        argument=$argv[index]
        case $argument in
            --config|-c)
                (( index += 2 ))
                ;;
            profile)
                (( index++ ))
                while (( index <= $# )); do
                    argument=$argv[index]
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

function __zetta_should_use_owner {
    (( $# == 0 )) && return 0

    case $argv[1] in
        pane|attention|notify|theme|overlay)
            return 0
            ;;
        cmd)
            (( $# >= 2 )) || return 1
            case $argv[2] in
                --list|-l|--help|-h)
                    return 1
                    ;;
            esac
            local index argument
            for (( index = 3; index <= $#; index++ )); do
                argument=$argv[index]
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
            case $argv[2] in
                add|remove|open)
                    return 0
                    ;;
            esac
            return 1
            ;;
        tabicon)
            case $argv[2] in
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

    local index=1 argument
    while (( index <= $# )); do
        argument=$argv[index]
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
    function zetta {
        if __zetta_should_use_owner "$@"; then
            __zetta_run_owner "$@"
        else
            __zetta_run_path "$@"
        fi
    }
fi

if (( ! $+functions[__zetta_report_cwd] )); then
    typeset -g __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    typeset -g __ZETTA_LIFECYCLE_TRACKING_VERSION=3
    if [[ -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} ]]; then
        typeset -g __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
    else
        typeset -g __ZETTA_LIFECYCLE_TRACKING_ENABLED=0
    fi
    typeset -g __ZETTA_COMMAND_STARTED=0
    function __zetta_report_tracking_ready() {
        (( __ZETTA_LIFECYCLE_TRACKING_ENABLED )) || return
        printf '\033]2;zetta-event:tracking-ready\033\\'
    }
    function __zetta_report_preexec() {
        (( __ZETTA_LIFECYCLE_TRACKING_ENABLED )) || return
        __ZETTA_COMMAND_STARTED=1
        printf '\033]2;zetta-event:command-started:%s\033\\' "$1"
    }
    function __zetta_report_cwd() {
        local zetta_status=$?
        if (( __ZETTA_LIFECYCLE_TRACKING_ENABLED && __ZETTA_COMMAND_STARTED )); then
            printf '\033]2;zetta-event:command-finished:%s\033\\' "$zetta_status"
            __ZETTA_COMMAND_STARTED=0
        fi
        [[ "$PWD" == /* ]] && printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
        return $zetta_status
    }
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd __zetta_report_cwd
    (( __ZETTA_LIFECYCLE_TRACKING_ENABLED )) && add-zsh-hook preexec __zetta_report_preexec
    __zetta_report_tracking_ready
fi

# Upgrade a shell that loaded an older CWD-only integration. The old script
# already registered __zetta_report_cwd with precmd, so redefining that
# function upgrades the existing hook without duplicating it.
if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3 ||
    ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} &&
        ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then
    typeset -g __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    typeset -g __ZETTA_LIFECYCLE_TRACKING_VERSION=3
    if [[ -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} ]]; then
        typeset -g __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
        typeset -g __ZETTA_COMMAND_STARTED=0
        function __zetta_report_tracking_ready() {
            (( __ZETTA_LIFECYCLE_TRACKING_ENABLED )) || return
            printf '\033]2;zetta-event:tracking-ready\033\\'
        }
        function __zetta_report_preexec() {
            (( __ZETTA_LIFECYCLE_TRACKING_ENABLED )) || return
            __ZETTA_COMMAND_STARTED=1
            printf '\033]2;zetta-event:command-started:%s\033\\' "$1"
        }
        function __zetta_report_cwd() {
            local zetta_status=$?
            if (( __ZETTA_LIFECYCLE_TRACKING_ENABLED && __ZETTA_COMMAND_STARTED )); then
                printf '\033]2;zetta-event:command-finished:%s\033\\' "$zetta_status"
                __ZETTA_COMMAND_STARTED=0
            fi
            [[ "$PWD" == /* ]] && printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
            return $zetta_status
        }
        autoload -Uz add-zsh-hook
        (( ${precmd_functions[(I)__zetta_report_cwd]:-0} == 0 )) &&
            add-zsh-hook precmd __zetta_report_cwd
        (( ${preexec_functions[(I)__zetta_report_preexec]:-0} == 0 )) &&
            add-zsh-hook preexec __zetta_report_preexec
        __zetta_report_tracking_ready
    else
        typeset -g __ZETTA_LIFECYCLE_TRACKING_ENABLED=0
    fi
fi

if (( ! ${+EDITOR} )); then
    export EDITOR='zetta vi'
fi

if (( ! $+commands[vi] && ! $+aliases[vi] && ! $+functions[vi] && ! $+builtins[vi] )); then
    function vi { zetta vi "$@"; }
    _zetta_vi_missing=1
else
    _zetta_vi_missing=${_zetta_vi_missing:-0}
fi

function zvi { zetta vi "$@"; }

if (( ! $+commands[mosh] && ! $+aliases[mosh] && ! $+functions[mosh] && ! $+builtins[mosh] )); then
    function mosh { zetta mosh "$@"; }
    _zetta_mosh_missing=1
else
    _zetta_mosh_missing=${_zetta_mosh_missing:-0}
fi

# ZETTA_WORKTREE_INTEGRATION_BEGIN
function zwt {
    case $1 in
        new)
            local worktree_path path_only_arg
            local -a operation_args=("${@[2,-1]}")
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
                worktree_path=$(command zwt new "${operation_args[@]}") || return
            else
                worktree_path=$(command zwt new --path-only "${operation_args[@]}") || return
            fi
            [[ -n $worktree_path ]] || return 1
            builtin cd -- "$worktree_path"
            ;;
        done)
            local worktree_path path_only_arg
            local -a operation_args=("${@[2,-1]}")
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
                worktree_path=$(command zwt done "${operation_args[@]}") || return
            else
                worktree_path=$(command zwt done --path-only "${operation_args[@]}") || return
            fi
            [[ -n $worktree_path ]] || return 1
            builtin cd -- "$worktree_path"
            ;;
        abort)
            local worktree_path path_only_arg
            local -a operation_args=("${@[2,-1]}")
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
                worktree_path=$(command zwt abort "${operation_args[@]}") || return
            else
                worktree_path=$(command zwt abort --path-only "${operation_args[@]}") || return
            fi
            [[ -n $worktree_path ]] || return 1
            builtin cd -- "$worktree_path"
            ;;
        *)
            command zwt "$@"
            ;;
    esac
}
# ZETTA_WORKTREE_INTEGRATION_END

ztftp() { zetta tftp "$@"; }
zntfy() { zetta notify "$@"; }

# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
unfunction zcopy zpaste 2>/dev/null
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
        function pbcopy { command zcopy "$@"; }
        function pbpaste { command zpaste "$@"; }
        ;;
esac
# ZETTA_CLIPBOARD_INTEGRATION_END


# Register now when completion exists; otherwise defer compinit to a widget.
function __zetta_lazy_complete {
    (( ${__zetta_completions_loading:-0} == 0 )) || return
    if (( ${__zetta_completions_loaded:-0} == 0 )); then
        local payload
        __zetta_completions_loading=1
        if payload=$(zetta init zsh --completions) && [[ -n $payload ]] && eval "$payload"; then
            __zetta_completions_loaded=1
        fi
        __zetta_completions_loading=0
        (( ${__zetta_completions_loaded:-0} )) || return
    fi
    local handler=${_comps[${words[1]}]}
    [[ -n $handler && $handler != __zetta_lazy_complete ]] && "$handler" "$@"
}
function __zetta_register_lazy_completions {
    (( $+functions[compdef] )) || return 1
    if (( ${__zetta_completions_loaded:-0} == 0 )); then
        compdef __zetta_lazy_complete zetta zosh zvi ztftp zntfy
        (( _zetta_vi_missing )) && compdef __zetta_lazy_complete vi
        (( _zetta_mosh_missing )) && compdef __zetta_lazy_complete mosh
# ZETTA_WORKTREE_INTEGRATION_BEGIN
        compdef __zetta_lazy_complete zwt
# ZETTA_WORKTREE_INTEGRATION_END
# ZETTA_ZMUX_INTEGRATION_BEGIN
        compdef __zetta_lazy_complete zmux
# ZETTA_ZMUX_INTEGRATION_END
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
        compdef __zetta_lazy_complete zcopy zpaste
        case "$OSTYPE" in darwin*) ;; *) compdef __zetta_lazy_complete pbcopy pbpaste ;; esac
# ZETTA_CLIPBOARD_INTEGRATION_END
    fi
    return 0
}
function __zetta_restore_completion_widgets {
    local widget
    for widget in $__zetta_completion_widgets; do
        # A later startup file may have installed a custom widget. Keep it.
        if [[ ${widgets[$widget]} == user:__zetta_initialize_completion ]]; then
            zle -A .zetta-$widget $widget
        fi
        zle -D .zetta-$widget
    done
    __zetta_completion_widgets=()
}
function __zetta_initialize_completion {
    local requested=$WIDGET
    __zetta_restore_completion_widgets
    if (( ! $+functions[compdef] )); then
        autoload -Uz compinit
        compinit
    fi
    __zetta_register_lazy_completions
    zle "$requested"
}
function __zetta_completion_prompt_hook {
    if (( $+functions[compdef] )); then
        __zetta_restore_completion_widgets
        __zetta_register_lazy_completions
        precmd_functions=(${precmd_functions:#__zetta_completion_prompt_hook})
    fi
}
if ! __zetta_register_lazy_completions; then
    if [[ -o interactive && ${__zetta_completion_widgets_installed:-0} == 0 ]]; then
        __zetta_completion_widgets_installed=1
        typeset -ga __zetta_completion_widgets=()
        zmodload zsh/zleparameter
        for __zetta_widget in complete-word expand-or-complete expand-or-complete-prefix menu-complete menu-expand-or-complete reverse-menu-complete; do
            if [[ ${widgets[$__zetta_widget]} == builtin ]]; then
                zle -A $__zetta_widget .zetta-$__zetta_widget
                zle -N $__zetta_widget __zetta_initialize_completion
                __zetta_completion_widgets+=($__zetta_widget)
            fi
        done
        unset __zetta_widget
        autoload -Uz add-zsh-hook
        add-zsh-hook precmd __zetta_completion_prompt_hook
    fi
fi
