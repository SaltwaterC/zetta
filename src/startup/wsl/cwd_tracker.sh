marker="${ZETTA_CWD_TRACKING_FILE:-}"
unset ZETTA_CWD_TRACKING_FILE
# The nonce `zetta-cmd;` markers carry. Only the shells integrated below are
# handed it back, and each moves it into a shell variable, so the programs a
# pane runs do not inherit it.
zetta_marker_nonce="${__ZETTA_COMMAND_MARKER_NONCE:-}"
unset __ZETTA_COMMAND_MARKER_NONCE
shell="${SHELL:-}"
if [ ! -x "$shell" ]; then
    shell="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)"
fi
[ -x "$shell" ] || shell=/bin/sh
# Windows-side process inspection can't see into the WSL VM's own process
# namespace, so the tab title can't be derived from the host process tree the
# way it is for native Windows shells. Report it explicitly instead: a
# `zetta-cmd;<nonce>:<value>` title marker carrying the shell name at idle, or
# the command about to run, mirrored by `foreground_command_marker` in
# crates/terminal/src/terminal.rs.
export ZETTA_SHELL_NAME="${shell##*/}"

case "${shell##*/}" in
    bash)
        # Bash 3.2 misparses case patterns in a heredoc inside $(...).
        # Define the producer first so the substitution only parses its call.
        zetta_bash_prompt() {
            cat <<'ZETTA_BASH_PROMPT'
# zetta: bash-tracker
ZETTA_BASH_PROMPT
        }
        # The tracker runs once, at the first prompt, and replaces itself; it
        # picks up the hook set aside here, and whatever the rc files add
        # around it, and runs them at every prompt after its own.
        export ZETTA_ORIGINAL_PROMPT_COMMAND="${PROMPT_COMMAND-}"
        PROMPT_COMMAND="$(zetta_bash_prompt)"
        export PROMPT_COMMAND
        export __ZETTA_COMMAND_MARKER_NONCE="$zetta_marker_nonce"
        exec "$shell" -l
        ;;
    fish)
        export __ZETTA_COMMAND_MARKER_NONCE="$zetta_marker_nonce"
        exec "$shell" -l -C 'if set -q __ZETTA_COMMAND_MARKER_NONCE; set -g __zetta_marker_nonce "$__ZETTA_COMMAND_MARKER_NONCE"; set -e __ZETTA_COMMAND_MARKER_NONCE; end; set -g __ZETTA_COMMAND_STARTED 0; function __zetta_report_cwd --on-event fish_prompt; set -l command_status $status; if test "$__ZETTA_COMMAND_STARTED" = 1; printf "\033]2;zetta-event;%s:command-finished:%s\033\\" "$__zetta_marker_nonce" "$command_status"; set -g __ZETTA_COMMAND_STARTED 0; end; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\" "$PWD"; if test -n "$__zetta_marker_nonce"; printf "\033]2;zetta-cwd;%s:%s\033\\" "$__zetta_marker_nonce" "$PWD"; else; printf "\033]2;zetta-cwd:%s\033\\" "$PWD"; end; end; printf "\033]2;zetta-cmd;%s:%s\033\\" "$__zetta_marker_nonce" "$ZETTA_SHELL_NAME"; end; function __zetta_report_preexec --on-event fish_preexec; set -g __ZETTA_COMMAND_STARTED 1; printf "\033]2;zetta-event;%s:command-started:%s\033\\" "$__zetta_marker_nonce" "$argv[1]"; printf "\033]2;zetta-cmd;%s:%s\033\\" "$__zetta_marker_nonce" "$argv[1]"; end; printf "\033]2;zetta-event;%s:tracking-ready\033\\" "$__zetta_marker_nonce"; if test -n "$ZETTA_HOST_EXECUTABLE"; and not functions -q __zetta_at_subcommand; $ZETTA_HOST_EXECUTABLE init fish </dev/null | source; end'
        ;;
    zsh)
        integration_zdotdir="$(mktemp -d "${TMPDIR:-/tmp}/zetta-zsh-XXXXXX" 2>/dev/null || true)"
        if [ -n "$integration_zdotdir" ]; then
            export ZETTA_ORIGINAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
            export ZETTA_INTEGRATION_ZDOTDIR="$integration_zdotdir"
            cat > "$integration_zdotdir/.zshenv" <<'ZETTA_ZSHENV'
typeset -g __zetta_marker_nonce="${__ZETTA_COMMAND_MARKER_NONCE-}"
unset __ZETTA_COMMAND_MARKER_NONCE
ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"

if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]]; then
    function zetta { command "$ZETTA_HOST_EXECUTABLE" "$@"; }
fi

function __zetta_report_cwd() {
    local zetta_status=$?
    if (( __ZETTA_COMMAND_STARTED )); then
        printf '\033]2;zetta-event;%s:command-finished:%s\033\\' "$__zetta_marker_nonce" "$zetta_status"
        __ZETTA_COMMAND_STARTED=0
    fi
    if [[ "$PWD" == /* ]]; then
        printf '\033]7;file://localhost%s\033\\' "$PWD"
        if [[ -n ${__zetta_marker_nonce-} ]]; then
            printf '\033]2;zetta-cwd;%s:%s\033\\' "$__zetta_marker_nonce" "$PWD"
        else
            printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
        fi
    fi
    printf '\033]2;zetta-cmd;%s:%s\033\\' "$__zetta_marker_nonce" "$ZETTA_SHELL_NAME"
    return $zetta_status
}
function __zetta_report_preexec() {
    __ZETTA_COMMAND_STARTED=1
    printf '\033]2;zetta-event;%s:command-started:%s\033\\' "$__zetta_marker_nonce" "$1"
    printf '\033]2;zetta-cmd;%s:%s\033\\' "$__zetta_marker_nonce" "$1"
}
typeset -g __ZETTA_COMMAND_STARTED=0
function __zetta_load_shell_integration() {
    add-zsh-hook -d precmd __zetta_load_shell_integration
    if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]] && (( ! $+functions[_zetta] )); then
        eval "$("$ZETTA_HOST_EXECUTABLE" init zsh </dev/null)"
    fi
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_load_shell_integration
add-zsh-hook precmd __zetta_report_cwd
add-zsh-hook preexec __zetta_report_preexec
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR
printf '\033]2;zetta-event;%s:tracking-ready\033\\' "$__zetta_marker_nonce"
ZETTA_ZSHENV
            ZDOTDIR="$integration_zdotdir"
            export ZDOTDIR
            export __ZETTA_COMMAND_MARKER_NONCE="$zetta_marker_nonce"
            exec "$shell" -l
        fi
        ;;
esac

# Shells without an injection mechanism retain the legacy tracker.
parent=$$
if [ -n "$marker" ]; then
    (
        previous=
        while kill -0 "$parent" 2>/dev/null; do
            cwd="$(readlink "/proc/$parent/cwd" 2>/dev/null)" || break
            if [ "$cwd" != "$previous" ]; then
                printf '%s\n' "$cwd" > "${marker}.tmp" && mv -f "${marker}.tmp" "$marker"
                previous="$cwd"
            fi
            sleep 0.1
        done
        rm -f "$marker" "${marker}.tmp"
    ) </dev/null >/dev/null 2>&1 &
fi
exec "$shell" -l
