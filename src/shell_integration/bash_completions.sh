# ZETTA_ZMUX_INTEGRATION_BEGIN
    _zetta_complete_mux_session_ids() {
        COMPREPLY=()
        local session_id
        local -a mux_list_command=(zetta mux list)
        if [[ ${COMP_WORDS[0]##*/} == zmux ]]; then
            mux_list_command=(zmux list)
        fi
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <("${mux_list_command[@]}" 2>/dev/null | awk '$1 == "reconnect" && $2 == "id:" && $3 ~ /^[0-9]+:[0-9]+:[0-9]+$/ { print $3 }')
    }
# ZETTA_ZMUX_INTEGRATION_END

    _zetta_complete_ssh_targets() {
        COMPREPLY=()
        local config="${HOME:-}/.ssh/config"
        [[ -r $config ]] || return
        local host
        while IFS= read -r host; do
            [[ $host == "$current"* ]] && COMPREPLY+=("$host")
        done < <(awk '
            /^[[:space:]]*[Hh][Oo][Ss][Tt][[:space:]]+/ {
                for (field = 2; field <= NF; field++)
                    if ($field !~ /^!/ && $field !~ /[*?]/)
                        print $field
            }
        ' "$config" 2>/dev/null)
    }

# ZETTA_ZMUX_INTEGRATION_BEGIN
    _zetta_complete_remote_mux_session_ids() {
        COMPREPLY=()
        local target=$1 port=$2 session_id
        local -a ssh_command=(ssh -T -o BatchMode=yes -o ConnectTimeout=3)
        [[ -n $port ]] && ssh_command+=(-p "$port")
        while IFS= read -r session_id; do
            [[ $session_id =~ ^[0-9]+$ && $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <("${ssh_command[@]}" "$target" zmux list --ids-only 2>/dev/null)
    }

    # `mux attach`'s operands: an SSH host until one is given, by operand
    # or --ssh-target, then that host's session IDs.
    _zetta_complete_mux_attach() {
        local target='' port='' token index=1
        while [[ ${COMP_WORDS[index]} != attach ]] && (( index < COMP_CWORD )); do
            (( index++ ))
        done
        for (( index++; index < COMP_CWORD; index++ )); do
            token=${COMP_WORDS[index]}
            case $token in
                --ssh-target|-H) (( ++index < COMP_CWORD )) && target=${COMP_WORDS[index]} ;;
                --ssh-target=*) target=${token#*=} ;;
                --port|-p) (( ++index < COMP_CWORD )) && port=${COMP_WORDS[index]} ;;
                --port=*) port=${token#*=} ;;
                --identity|-i|--protocol|-P) (( index++ )) ;;
                -*) ;;
                *) [[ -n $target ]] || target=$token ;;
            esac
        done
        if [[ -n $target ]]; then
            _zetta_complete_remote_mux_session_ids "$target" "$port"
        else
            _zetta_complete_ssh_targets
        fi
    }
# ZETTA_ZMUX_INTEGRATION_END

# ZETTA_ZMUX_INTEGRATION_BEGIN
    _zetta_complete_mux_restorable_ids() {
        COMPREPLY=()
        local session_id
        local -a mux_list_command=(zetta mux list)
        if [[ ${COMP_WORDS[0]##*/} == zmux ]]; then
            mux_list_command=(zmux list)
        fi
        while IFS= read -r session_id; do
            [[ $session_id == "$current"* ]] && COMPREPLY+=("$session_id")
        done < <("${mux_list_command[@]}" 2>/dev/null | awk '$1 == "resume" && $2 == "id:" && $3 ~ /^[0-9]+$/ { print $3 }')
    }
# ZETTA_ZMUX_INTEGRATION_END

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
# ZETTA_WORKTREE_INTEGRATION_END

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

# ZETTA_COMMAND_TREE
# `vi` and `mosh` are `zvi` and `zosh` where the startup script wraps them.
# (An `if`, not `&&`: the payload's status is the last command's.)
if [[ ${__ZETTA_VI_WRAPPER:-0} == 1 ]]; then
    complete -F _zetta_entry_zvi vi
fi
# ZETTA_ZOSH_INTEGRATION_BEGIN
if [[ ${__ZETTA_MOSH_WRAPPER:-0} == 1 ]]; then
    complete -F _zetta_entry_zosh mosh
fi
# ZETTA_ZOSH_INTEGRATION_END
