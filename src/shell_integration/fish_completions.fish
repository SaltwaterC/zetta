function __zetta_pane_labels
    zetta pane --list 2>/dev/null
end

function __zetta_run_pane_labels
    set -l current (commandline -ct)
    set -l prefix ''
    set -l partial $current
    set -l selected
    if string match -q '*,*' -- "$current"
        set prefix (string replace -r '[^,]*$' '' -- "$current")
        set partial (string replace -r '^.*,' '' -- "$current")
        set selected (string split ',' -- (string replace -r ',[^,]*$' '' -- "$current"))
    end
    for label in (__zetta_pane_labels)
        string match -q -- "$partial*" "$label"; or continue
        contains -- "$label" $selected; and continue
        printf '%s%s\n' "$prefix" "$label"
    end
end

# zetta-default/zetta-ok/zetta-alarm/zetta-gong are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
function __zetta_sound_names
    switch $__zetta_platform
        case Darwin
            printf '%s\n' zetta-default zetta-ok zetta-alarm zetta-gong \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
        case '*'
            printf '%s\n' zetta-default zetta-ok zetta-alarm zetta-gong bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
    end
end

# ZETTA_ZMUX_INTEGRATION_BEGIN
function __zetta_mux_session_ids
    set -l command_name zetta
    set -l command_arguments mux list
    set -l words (commandline -opc)
    if test "$words[1]" = zmux
        set command_name zmux
        set command_arguments list
    end
    $command_name $command_arguments 2>/dev/null | awk '$1 == "reconnect" && $2 == "id:" && $3 ~ /^[0-9]+:[0-9]+:[0-9]+$/ { print $3 }'
end

function __zetta_mux_restorable_ids
    set -l command_name zetta
    set -l command_arguments mux list
    set -l words (commandline -opc)
    if test "$words[1]" = zmux
        set command_name zmux
        set command_arguments list
    end
    $command_name $command_arguments 2>/dev/null | awk '$1 == "resume" && $2 == "id:" && $3 ~ /^[0-9]+$/ { print $3 }'
end
# ZETTA_ZMUX_INTEGRATION_END

function __zetta_ssh_targets
    set -l config "$HOME/.ssh/config"
    test -r "$config"; or return
    awk '
        /^[[:space:]]*[Hh][Oo][Ss][Tt][[:space:]]+/ {
            for (field = 2; field <= NF; field++)
                if ($field !~ /^!/ && $field !~ /[*?]/)
                    print $field
        }
    ' "$config" 2>/dev/null
end

# Whether a Mosh host positional has already been given. Everything after the
# host is sent to the Mosh server, so no further candidates may be offered
# once it is on the command line.
# ZETTA_ZMUX_INTEGRATION_BEGIN
# `mux attach`'s operands: an SSH host until one is given, by operand or
# --ssh-target, then that host's session IDs.
function __zetta_mux_attach
    set -l words (commandline -opc)
    set -l index (contains -i -- attach $words)
    set -l count (count $words)
    set -l target ''
    set -l port ''
    while test $index -lt $count
        set index (math $index + 1)
        set -l token $words[$index]
        switch $token
            case --ssh-target -H
                set index (math $index + 1)
                test $index -le $count; and set target $words[$index]
            case '--ssh-target=*'
                set target (string replace -- '--ssh-target=' '' $token)
            case --port -p
                set index (math $index + 1)
                test $index -le $count; and set port $words[$index]
            case '--port=*'
                set port (string replace -- '--port=' '' $token)
            case --identity -i --protocol -P
                set index (math $index + 1)
            case '-*'
            case '*'
                test -n "$target"; or set target $token
        end
    end
    if test -n "$target"
        set -l ssh_command ssh -T -o BatchMode=yes -o ConnectTimeout=3
        test -n "$port"; and set ssh_command $ssh_command -p $port
        set -l prefix (commandline -ct)
        command $ssh_command $target zmux list --ids-only 2>/dev/null \
            | string match -r '^[0-9]+$' \
            | string match -- "$prefix*"
    else
        __zetta_ssh_targets
    end
end
# ZETTA_ZMUX_INTEGRATION_END

# ZETTA_COMMAND_TREE
# `mosh` is `zosh` where the startup script wraps it.
# ZETTA_ZOSH_INTEGRATION_BEGIN
if set -q __ZETTA_MOSH_WRAPPER
    complete -c mosh -f -a '(__zetta_tree_complete zosh)'
end
# ZETTA_ZOSH_INTEGRATION_END
