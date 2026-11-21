: __zetta_loader_begin
# Zetta hands this whole script to Bash as PROMPT_COMMAND. It runs once, at
# the first prompt, and replaces itself with `__zetta_prompt_command`; whatever
# else PROMPT_COMMAND held — the hook Zetta found in the environment, and
# anything the rc files put before or after this script — runs inside that
# function from then on.
if [[ -n ${__ZETTA_COMMAND_MARKER_NONCE+x} ]]; then
    __zetta_marker_nonce=$__ZETTA_COMMAND_MARKER_NONCE
    unset __ZETTA_COMMAND_MARKER_NONCE
fi
__zetta_at_prompt=0
__zetta_command_started=0
# The hook Zetta found in the environment. MSYS2 and Cygwin have it set here
# as a quoted word; WSL, whose hook is only known inside the distribution,
# has wsl_bash_loader.bash here instead.
__zetta_user_prompt_command=
# zetta: user-prompt-command
__zetta_preexec() {
    [[ "$__zetta_at_prompt" == 1 ]] || return
    __zetta_at_prompt=0
    case "$BASH_COMMAND" in
        __zetta_prompt_command|__zetta_precmd|__zetta_mark_prompt|__zetta_loader_end) return ;;
    esac
    __zetta_command_started=1
    printf '\033]2;zetta-event;%s:command-started:%s\033\\' "${__zetta_marker_nonce-}" "$BASH_COMMAND"
    printf '\033]2;zetta-cmd;%s:%s\033\\' "${__zetta_marker_nonce-}" "$BASH_COMMAND"
}
__zetta_precmd() {
    local status=$?
    if [[ "$__zetta_command_started" == 1 ]]; then
        printf '\033]2;zetta-event;%s:command-finished:%s\033\\' "${__zetta_marker_nonce-}" "$status"
        __zetta_command_started=0
    fi
    case "$PWD" in
        /*)
            # zetta: file-uri
            if [[ -n ${__zetta_marker_nonce-} ]]; then
                printf '\033]2;zetta-cwd;%s:%s\033\\' "$__zetta_marker_nonce" "$PWD"
            else
                printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
            fi
            ;;
    esac
    printf '\033]2;zetta-cmd;%s:bash\033\\' "${__zetta_marker_nonce-}"
    return "$status"
}
__zetta_mark_prompt() {
    __zetta_at_prompt=1
}
# The DEBUG trap does not reach inside a function, so no hook run from here is
# mistaken for a command the user typed. `__zetta_precmd` hands the command's
# status back, so the hooks see the same `$?` they would have.
__zetta_prompt_command() {
    __zetta_precmd
    eval "$__zetta_original_prompt_command"
    __zetta_mark_prompt
}
# Ends the load. It is the script's last command, so it is also the only
# place the first prompt can be marked: a command after the mark would be
# reported as the user's. A hook an rc file appended still runs after it, at
# the top level, so then the prompt is left unmarked, which costs only the
# first command's events.
__zetta_loader_end() {
    [[ -n $__zetta_after ]] || __zetta_mark_prompt
    unset __zetta_before __zetta_after
    unset -f __zetta_loader_end
}
# What the rc files added around this script. The prefix has already run for
# this prompt; anything after the final `__zetta_loader_end` is still to run.
__zetta_before=${PROMPT_COMMAND%%': __zetta_loader_begin'*}
__zetta_after=${PROMPT_COMMAND##*__zetta_loader_end}
__zetta_after=${__zetta_after#"${__zetta_after%%[!$' \t\n;']*}"}
__zetta_original_prompt_command=$__zetta_before$'\n'$__zetta_user_prompt_command$'\n'$__zetta_after
PROMPT_COMMAND=__zetta_prompt_command
trap '__zetta_preexec' DEBUG
printf '\033]2;zetta-event;%s:tracking-ready\033\\' "${__zetta_marker_nonce-}"
__zetta_precmd
eval "$__zetta_user_prompt_command"
__zetta_loader_end
