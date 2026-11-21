# Completes a command from its generated tree. Each node is a function
# `__zetta_node_<id>` that answers `subcommands` and `options` (as
# name<TAB>description lines), `late` (the subcommands that may follow its
# options), `repeatable`, `alone` (options valid only on their own),
# `operands`, `options-first` (whether the first operand ends the options),
# `rest` (what follows `--`, if the operands end at it), and `option WORD`:
# the long option WORD spells, then the kind of value it takes. See
# command_tree.rs.
function __zetta_tree_complete --argument-names node
    set -l words (commandline -opc)
    set -l current (commandline -ct)
    set -l pending
    set -l operands 0
    set -l given 0
    set -l ended 0
    set -l used
    set -l alone 0
    set -l instead 0
    set -l after
    # The configuration file given, which the runtime lists are read with.
    set -l config
    # The words after `--`, for a command line of their own.
    set -l rest
    for word in $words[2..-1]
        if test -n "$pending"
            test "$pending" = config; and set config --config $word
            set pending
        else if test $ended = 0; and test "$word" = --
            set ended 1
        else if test $ended = 1
            set -a rest $word
            set operands (math $operands + 1)
        else if string match -qr -- '^-.' $word
            set -l answer (__zetta_node_$node option $word)
            set -a used $answer[1]
            set given (math $given + 1)
            contains -- $answer[1] (__zetta_node_$node alone); and set alone 1
            # An option taking a command line takes the rest of the words.
            if string match -q 'attached:*' -- $answer[2]
                # `--option=VALUE`, its value given.
                test "$answer[2]" = attached:config
                and set config --config (string replace -r '^[^=]*=' '' -- $word)
            else if test "$answer[2]" = command
                set ended 1
                set after command
            else if test "$answer[2]" = instead
                set instead 1
            else if test "$answer[2]" = exits
                set alone 1
            else if test "$answer[2]" != nothing
                set pending $answer[2]
            end
        # A subcommand is only ever the first word after its parent, or one
        # of the parent's options.
        else if test $operands = 0; and contains -- $word (__zetta_tree_subcommands $node $given)
            set node {$node}__(string replace -a -- - _ $word)
            set given 0
        else
            set operands (math $operands + 1)
            set given (math $given + 1)
            test (__zetta_node_$node options-first) = 1; and set ended 1
        end
    end
    # Nothing follows an option valid only on its own.
    test $alone = 1; and return
    # The kind of the operand being typed; the last kind listed repeats.
    set -l kinds (__zetta_node_$node operands)
    set -l position (math $operands + 1)
    test $position -gt (count $kinds); and set position (count $kinds)
    set -l operand_kind $kinds[$position]
    # A switch given in place of the last operand leaves one fewer to give.
    test $instead = 1 -a $operands -ge (math (count $kinds) - 2); and set operand_kind nothing
    test -n "$after"; or set after (__zetta_node_$node rest)
    test $ended = 1 -a -n "$after"; and set operand_kind $after
    # For `--option=VALUE` being typed, the kind of that option's value.
    set -l attached
    if test $ended = 0; and string match -q -- '-*=*' $current
        set -l answer (__zetta_node_$node option $current)
        string match -q 'attached:*' -- $answer[2]
        and set attached (string replace 'attached:' '' -- $answer[2])
    end
    if test -n "$pending"
        __zetta_tree_values $pending $current $config
    else if test -n "$attached"
        # Its values, after the `--option=`.
        set -l prefix (string replace -r '=.*' '=' -- $current)
        __zetta_tree_values $attached (string replace -r '^[^=]*=' '' -- $current) $config |
            string replace -r -- '^' $prefix
    else if test "$operand_kind" = command
        # Completed as fish completes that command typed alone.
        __fish_complete_subcommand --commandline $rest $current
    else if test $ended = 0; and string match -q -- '-*' $current
        __zetta_tree_options $node $given $used
    else
        if test $ended = 0 -a $operands = 0
            set -l late (__zetta_node_$node late)
            for line in (__zetta_node_$node subcommands)
                test $given = 0; or contains -- (string replace -r '\t.*' '' -- $line) $late; and printf '%s\n' $line
            end
        end
        __zetta_tree_values $operand_kind $current $config
        if test $ended = 0; and contains -- $operand_kind nothing any
            __zetta_tree_options $node $given $used
        end
    end
end

# The names of the subcommands of node $argv[1] a word may name, once
# $argv[2] options or operands are given.
function __zetta_tree_subcommands --argument-names node given
    if test $given = 0
        __zetta_node_$node subcommands | string replace -r '\t.*' ''
    else
        __zetta_node_$node late
    end
end

function __zetta_tree_options --argument-names node given
    set -l used $argv[3..-1]
    set -l repeatable (__zetta_node_$node repeatable)
    set -l alone (__zetta_node_$node alone)
    for line in (__zetta_node_$node options)
        set -l name (string replace -r '\t.*' '' -- $line)
        contains -- $name $used; and not contains -- $name $repeatable; and continue
        test $given != 0; and contains -- $name $alone; and continue
        printf '%s\n' $line
    end
end

function __zetta_tree_values --argument-names kind current
    # The configuration file given, as `--config FILE`.
    set -l config $argv[3..-1]
    switch $kind
        case files config
            __fish_complete_path $current
        case directories
            __fish_complete_directories $current
        case 'words:*'
            string split ' ' -- (string replace 'words:' '' -- $kind)
        case 'zetta:*'
            zetta (string split ' ' -- (string replace 'zetta:' '' -- $kind)) 2>/dev/null
        case 'configured:*'
            zetta (string split ' ' -- (string replace 'configured:' '' -- $kind)) $config 2>/dev/null
        case 'call:*'
            eval (string replace 'call:' '' -- $kind)
    end
end
