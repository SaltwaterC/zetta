_zetta_run_pane_labels() {
    local value=${words[CURRENT]} prefix='' partial=$value label selected_label duplicate
    local -a selected=() labels=("${(@f)$(zetta pane --list 2>/dev/null)}")
    if [[ $value == *,* ]]; then
        prefix="${value%,*},"
        partial=${value##*,}
        selected=("${(@s/,/)${value%,*}}")
    fi
    for label in "${labels[@]}"; do
        [[ $label == "$partial"* ]] || continue
        duplicate=0
        for selected_label in "${selected[@]}"; do
            if [[ $selected_label == "$label" ]]; then
                duplicate=1
                break
            fi
        done
        (( duplicate )) || compadd -- "$prefix$label"
    done
}

# ZETTA_ZMUX_INTEGRATION_BEGIN
_zmux_session_ids() {
    local -a mux_list_command=(zetta mux list)
    [[ ${words[1]:t} == zmux ]] && mux_list_command=(zmux list)
    compadd -- "${(@f)$(${mux_list_command[@]} 2>/dev/null | awk '$1 == "reconnect" && $2 == "id:" && $3 ~ /^[0-9]+:[0-9]+:[0-9]+$/ { print $3 }')}"
}

_zmux_remote_session_ids() {
    local target=$1 port=$2
    local -a ssh_command=(ssh -T -o BatchMode=yes -o ConnectTimeout=3)
    [[ -n $port ]] && ssh_command+=(-p "$port")
    compadd -- "${(@f)$(${ssh_command[@]} "$target" zmux list --ids-only 2>/dev/null | awk -v prefix="$PREFIX" '$0 ~ /^[0-9]+$/ && index($0, prefix) == 1 { print }')}"
}

# `mux attach`'s operands: an SSH host until one is given, by operand or
# --ssh-target, then that host's session IDs.
_zmux_attach() {
    local target='' port='' token index=${words[(i)attach]}
    for (( index++; index < CURRENT; index++ )); do
        token=$words[index]
        case $token in
            --ssh-target|-H) (( ++index < CURRENT )) && target=$words[index] ;;
            --ssh-target=*) target=${token#*=} ;;
            --port|-p) (( ++index < CURRENT )) && port=$words[index] ;;
            --port=*) port=${token#*=} ;;
            --identity|-i|--protocol|-P) (( index++ )) ;;
            -*) ;;
            *) [[ -n $target ]] || target=$token ;;
        esac
    done
    if [[ -n $target ]]; then
        _zmux_remote_session_ids "$target" "$port"
    else
        _zetta_ssh_targets
    fi
}

_zmux_restorable_ids() {
    local -a mux_list_command=(zetta mux list)
    [[ ${words[1]:t} == zmux ]] && mux_list_command=(zmux list)
    compadd -- "${(@f)$(${mux_list_command[@]} 2>/dev/null | awk '$1 == "resume" && $2 == "id:" && $3 ~ /^[0-9]+$/ { print $3 }')}"
}
# ZETTA_ZMUX_INTEGRATION_END

_zetta_ssh_targets() {
    local config="${HOME:-}/.ssh/config"
    [[ -r $config ]] || return
    compadd -- "${(@f)$(awk '
        /^[[:space:]]*[Hh][Oo][Ss][Tt][[:space:]]+/ {
            for (field = 2; field <= NF; field++)
                if ($field !~ /^!/ && $field !~ /[*?]/)
                    print $field
        }
    ' "$config" 2>/dev/null)}"
}

# zetta-default/zetta-ok/zetta-alarm/zetta-gong are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
_zetta_sound_names() {
    case "$OSTYPE" in
        darwin*)
            compadd -- zetta-default zetta-ok zetta-alarm zetta-gong \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
            ;;
        msys*|cygwin*|win32*)
            compadd -- zetta-default zetta-ok zetta-alarm zetta-gong Default IM Mail Reminder SMS
            ;;
        *)
            compadd -- zetta-default zetta-ok zetta-alarm zetta-gong bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
            ;;
    esac
}

# ZETTA_WORKTREE_INTEGRATION_BEGIN
_zetta_worktree_commits() {
    local current_branch source_branch split_point
    current_branch=$(git branch --show-current 2>/dev/null) || return
    [[ -n $current_branch ]] || return
    source_branch=$(git config --local --get "wtbranch.${current_branch}.base" 2>/dev/null) || return
    [[ -n $source_branch ]] || return
    split_point=$(git merge-base "refs/heads/${current_branch}" "refs/heads/${source_branch}" 2>/dev/null) || return
    [[ -n $split_point ]] || return
    compadd -- "${(@f)$(git rev-list --reverse "${split_point}..refs/heads/${source_branch}" 2>/dev/null)}"
}
# ZETTA_WORKTREE_INTEGRATION_END

# ZETTA_COMMAND_TREE
# `vi` and `mosh` are `zvi` and `zosh` where the startup script wraps them.
# (An `if`, not `&&`: the payload's status is the last command's.)
if (( _zetta_vi_missing )); then
    compdef _zetta_entry_zvi vi
fi
# ZETTA_ZOSH_INTEGRATION_BEGIN
if (( _zetta_mosh_missing )); then
    compdef _zetta_entry_zosh mosh
fi
# ZETTA_ZOSH_INTEGRATION_END
