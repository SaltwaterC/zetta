# Zetta's startup history filter runs from .zshenv, before the first injected
# command reaches the interactive history mechanism.
if [[ ${ZETTA_ZSH_ORIGINAL_ZDOTDIR_SET:-0} == 1 ]]; then
    ZDOTDIR="$ZETTA_ZSH_ORIGINAL_ZDOTDIR"
    __zetta_original_zdotdir="$ZDOTDIR"
else
    unset ZDOTDIR
    __zetta_original_zdotdir="$HOME"
fi
[[ -r "$__zetta_original_zdotdir/.zshenv" ]] &&
    source "$__zetta_original_zdotdir/.zshenv"

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

command rm -f -- "$ZETTA_ZSH_HISTORY_ZDOTDIR/.zshenv" 2>/dev/null
command rmdir -- "$ZETTA_ZSH_HISTORY_ZDOTDIR" 2>/dev/null
unset ZETTA_ZSH_ORIGINAL_ZDOTDIR ZETTA_ZSH_ORIGINAL_ZDOTDIR_SET \
    ZETTA_ZSH_HISTORY_ZDOTDIR __zetta_original_zdotdir
