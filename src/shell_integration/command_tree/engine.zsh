# Completes a command from its generated tree. Each node is a function
# `_zetta_node_<id>` that sets the node's subcommands (as `name:description`,
# and those that may follow its options by name), long options (as
# `name:description`), repeatable options, options valid only alone, operand
# kinds, whether the first operand ends the options, and what follows `--`,
# and, given a word, the long option it spells and the kind of value that
# option takes. See command_tree.rs.
_zetta_tree_complete() {
    local node=$1 word index pending='' operands=0 given=0 ended=0 rest=0 alone=0 instead=0 after=''
    local -a used names config spec_subcommands spec_late spec_options spec_repeatable spec_alone
    local -a spec_operands
    local spec_option spec_value spec_rest spec_options_first operand_kind
    for (( index = 2; index < CURRENT; index++ )); do
        word=${words[index]}
        if [[ -n $pending ]]; then
            # The runtime lists are read with the configuration file given.
            [[ $pending == config ]] && config=(--config "$word")
            pending=''
        elif (( ! ended )) && [[ $word == -- ]]; then
            ended=1 rest=$(( index + 1 ))
        elif (( ! ended )) && [[ $word == -?* ]]; then
            _zetta_node_$node "$word"
            used+=($spec_option)
            (( ++given ))
            (( ${spec_alone[(Ie)$spec_option]} )) && alone=1
            # An option taking a command line takes the rest of the words.
            if [[ $spec_value == attached:* ]]; then
                # `--option=VALUE`, its value given.
                [[ $spec_value == attached:config ]] && config=(--config "${word#*=}")
            elif [[ $spec_value == command ]]; then
                ended=1 rest=$(( index + 1 )) after=command
            elif [[ $spec_value == instead ]]; then
                instead=1
            elif [[ $spec_value == exits ]]; then
                alone=1
            elif [[ $spec_value != nothing ]]; then
                pending=$spec_value
            fi
        else
            _zetta_node_$node
            # A subcommand is only ever the first word after its parent, or
            # one of the parent's options. Outside (( )), which would join
            # the names into one string.
            names=(${spec_subcommands%%:*})
            (( given )) && names=($spec_late)
            if (( ! ended && ! operands && ${names[(Ie)$word]} )); then
                node=${node}__${word//-/_} given=0
            else
                (( operands++, ++given ))
                (( spec_options_first )) && ended=1
            fi
        fi
    done
    # Nothing follows an option valid only on its own.
    (( alone )) && return
    _zetta_node_$node
    # The kind of the operand being typed; the last kind listed repeats.
    operand_kind=${spec_operands[operands < $#spec_operands ? operands + 1 : $#spec_operands]}
    # A switch given in place of the last operand leaves one fewer to give.
    (( instead && operands >= $#spec_operands - 2 )) && operand_kind=nothing
    (( ended )) && [[ -n ${after:=$spec_rest} ]] && operand_kind=$after
    if [[ -n $pending ]]; then
        _zetta_tree_values $pending
    elif (( ! ended )) && [[ ${words[CURRENT]} == -*=* ]] &&
        _zetta_node_$node "${words[CURRENT]}" && [[ $spec_value == attached:* ]]; then
        # `--option=VALUE`: its values, after the `--option=`.
        compset -P '*='
        _zetta_tree_values ${spec_value#attached:}
    elif (( ! ended )) && [[ ${words[CURRENT]} == -* ]]; then
        _zetta_tree_options
    else
        if (( ! ended && ! operands )); then
            (( given )) && spec_subcommands=(${(M)spec_subcommands:#(${(j:|:)~spec_late}):*})
            _describe -t commands command spec_subcommands
        fi
        _zetta_tree_values $operand_kind
        if (( ! ended )) && [[ $operand_kind == (nothing|any) ]]; then
            _zetta_tree_options
        fi
    fi
}

_zetta_tree_options() {
    local -a offered
    local item
    for item in $spec_options; do
        if (( ( ! ${used[(Ie)${item%%:*}]} || ${spec_repeatable[(Ie)${item%%:*}]} ) &&
            ( given == 0 || ! ${spec_alone[(Ie)${item%%:*}]} ) )); then
            offered+=($item)
        fi
    done
    _describe -t zetta-options option offered
}

_zetta_tree_values() {
    case $1 in
        files|config) _files ;;
        directories) _files -/ ;;
        words:*) compadd -- ${=${1#words:}} ;;
        zetta:*) compadd -- "${(@f)$(zetta ${=${1#zetta:}} 2>/dev/null)}" ;;
        configured:*) compadd -- "${(@f)$(zetta ${=${1#configured:}} $config 2>/dev/null)}" ;;
        call:*) ${1#call:} ;;
        # The words from index $rest on are a command line of their own:
        # shift them into place and let zsh complete it as if typed alone.
        command)
            words=("${(@)words[rest,-1]}")
            (( CURRENT -= rest - 1 ))
            _normal
            ;;
    esac
}
