if [[ -n ${ZETTA_ORIGINAL_ZDOTDIR+x} ]]; then
    ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
    export ZDOTDIR
else
    unset ZDOTDIR
fi
typeset -g __zetta_marker_nonce="${__ZETTA_COMMAND_MARKER_NONCE-}"
unset __ZETTA_COMMAND_MARKER_NONCE
original_zdotdir="${ZDOTDIR:-$HOME}"
[[ -r "$original_zdotdir/.zshenv" ]] && source "$original_zdotdir/.zshenv"

function __zetta_report_cwd() {
    local zetta_status=$?
    if (( __ZETTA_COMMAND_STARTED )); then
        printf '\033]2;zetta-event;%s:command-finished:%s\033\\' "$__zetta_marker_nonce" "$zetta_status"
        __ZETTA_COMMAND_STARTED=0
    fi
    if [[ "$PWD" == /* ]]; then
        # zetta: file-uri
        if [[ -n ${__zetta_marker_nonce-} ]]; then
            printf '\033]2;zetta-cwd;%s:%s\033\\' "$__zetta_marker_nonce" "$PWD"
        else
            printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
        fi
    fi
    printf '\033]2;zetta-cmd;%s:zsh\033\\' "$__zetta_marker_nonce"
    return $zetta_status
}
function __zetta_report_preexec() {
    __ZETTA_COMMAND_STARTED=1
    printf '\033]2;zetta-event;%s:command-started:%s\033\\' "$__zetta_marker_nonce" "$1"
    printf '\033]2;zetta-cmd;%s:%s\033\\' "$__zetta_marker_nonce" "$1"
}
typeset -g __ZETTA_COMMAND_STARTED=0
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR original_zdotdir
printf '\033]2;zetta-event;%s:tracking-ready\033\\' "$__zetta_marker_nonce"

