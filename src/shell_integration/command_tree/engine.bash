# Completes a command from its generated tree. Each node is a function
# `_zetta_node_<id>` that sets the node's subcommands (and those that may
# follow its options), long options, repeatable options, options valid only
# alone, operand kinds, whether the first operand ends the options, and what
# follows `--`, and, given a word, the long option it spells and the kind of
# value that option takes. See command_tree.rs.
_zetta_tree_complete() {
    local node=$1 current=${COMP_WORDS[COMP_CWORD]} word index
    local pending='' operands=0 given=0 ended=0 rest=0 alone=0 instead=0 used=' ' after='' offered
    local spec_subcommands spec_late spec_options spec_repeatable spec_alone spec_options_first
    local spec_rest spec_option spec_value operand_kind
    local -a spec_operands config=()
    COMPREPLY=()
    for (( index = 1; index < COMP_CWORD; index++ )); do
        word=${COMP_WORDS[index]}
        if [[ $word == = ]] && (( index > 1 )) && _zetta_tree_attached "${COMP_WORDS[index - 1]}"; then
            # Bash splits `--option=VALUE` at its `=`: the value comes next,
            # even for a switch that only takes one attached.
            [[ -n $pending ]] || pending=any
        elif [[ -n $pending ]]; then
            # The runtime lists are read with the configuration file given.
            [[ $pending == config ]] && config=(--config "$word")
            pending=''
        elif (( ! ended )) && [[ $word == -- ]]; then
            ended=1 rest=$(( index + 1 ))
        elif (( ! ended )) && [[ $word == -?* ]]; then
            "_zetta_node_$node" "$word"
            used+="$spec_option "
            (( ++given ))
            [[ " $spec_alone " == *" $spec_option "* ]] && alone=1
            # An option taking a command line takes the rest of the words.
            if [[ $spec_value == attached:* ]]; then
                # `--option=VALUE`, where bash was told not to split at `=`.
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
            "_zetta_node_$node"
            # A subcommand is only ever the first word after its parent, or
            # one of the parent's options.
            offered=$spec_subcommands
            (( given )) && offered=$spec_late
            if (( ! ended && ! operands )) && [[ " $offered " == *" $word "* ]]; then
                node=${node}__${word//-/_} given=0
            else
                (( operands++, ++given ))
                (( spec_options_first )) && ended=1
            fi
        fi
    done
    # Nothing follows an option valid only on its own.
    (( alone )) && return
    # The `=` of a split `--option=` is not part of the value being typed.
    if [[ $current == = ]] && _zetta_tree_attached "${COMP_WORDS[COMP_CWORD - 1]}"; then
        current=''
        [[ -n $pending ]] || pending=any
    fi
    "_zetta_node_$node"
    # The kind of the operand being typed; the last kind listed repeats.
    operand_kind=${spec_operands[operands < ${#spec_operands[@]} ? operands : ${#spec_operands[@]} - 1]}
    # A switch given in place of the last operand leaves one fewer to give.
    (( instead && operands >= ${#spec_operands[@]} - 2 )) && operand_kind=nothing
    (( ended )) && [[ -n ${after:=$spec_rest} ]] && operand_kind=$after
    if [[ -n $pending ]]; then
        _zetta_tree_values "$pending"
    elif (( ! ended )) && [[ $current == -*=* ]] && _zetta_tree_attached "${current%%=*}"; then
        # `--option=VALUE` in one word, as bash 3.2 leaves it, and bash 4 when
        # `=` is not in COMP_WORDBREAKS: its values. Readline replaces only
        # what follows a word break, so the `--option=` goes back in front of
        # them only where `=` is not one.
        local prefix=${current%%=*}=
        current=${current#*=}
        _zetta_tree_values "${spec_value#attached:}"
        [[ $COMP_WORDBREAKS == *=* ]] || COMPREPLY=("${COMPREPLY[@]/#/$prefix}")
    elif (( ! ended )) && [[ $current == -* ]]; then
        _zetta_tree_options
    else
        offered=$spec_subcommands
        (( given )) && offered=$spec_late
        (( ended || operands )) || COMPREPLY=( $(compgen -W "$offered" -- "$current") )
        _zetta_tree_values "$operand_kind"
        if (( ! ended )); then
            case $operand_kind in nothing|any) _zetta_tree_options ;; esac
        fi
    fi
}

# Whether option $1 of the current node accepts its value attached with `=`,
# leaving that value's kind in spec_value as `attached:KIND`.
_zetta_tree_attached() {
    "_zetta_node_$node" "$1="
    [[ $spec_value == attached:* ]]
}

_zetta_tree_options() {
    local option
    for option in $spec_options; do
        [[ $option == "$current"* ]] || continue
        [[ $used != *" $option "* || " $spec_repeatable " == *" $option "* ]] || continue
        [[ $given == 0 || " $spec_alone " != *" $option "* ]] || continue
        COMPREPLY+=("$option")
    done
}

# Adds the candidates for a value of kind $1 to COMPREPLY.
_zetta_tree_values() {
    local -a offered=("${COMPREPLY[@]}")
    local line lines=''
    COMPREPLY=()
    case $1 in
        files|config) lines=$(compgen -f -- "$current") ;;
        directories) lines=$(compgen -d -- "$current") ;;
        words:*) lines=$(compgen -W "${1#words:}" -- "$current") ;;
        zetta:*|configured:*)
            local -a arguments=(${1#*:})
            [[ $1 == configured:* ]] && arguments+=("${config[@]}")
            # The word as typed, less an opening quote and backslash escapes,
            # which compgen strips from a file name itself.
            local typed=${current#[\"\']}
            typed=${typed//\\/}
            while IFS= read -r line; do
                [[ $line == "$typed"* ]] && lines+=$line$'\n'
            done < <(zetta "${arguments[@]}" 2>/dev/null)
            ;;
        call:*) "${1#call:}" ;;
        command) _zetta_tree_command ;;
        *) COMPREPLY=("${offered[@]}"); return ;;
    esac
    # One candidate per line, taken as it is: a file or profile name may hold
    # spaces or glob characters, which word splitting would break apart.
    while IFS= read -r line; do
        [[ -n $line ]] && COMPREPLY+=("$line")
    done <<< "$lines"
    case $1 in
        files|config|directories) _zetta_tree_quote files ;;
        zetta:*|configured:*) _zetta_tree_quote names ;;
    esac
    COMPREPLY=( "${offered[@]}" "${COMPREPLY[@]}" )
}

# Has COMPREPLY's names, files when $1 is `files`, inserted quoted. Bash
# quotes what it inserts only when told the candidates are file names, which
# also marks those naming a directory with a slash: what file names want, but
# not a profile that happens to share a directory's name. Bash 3.2 cannot be
# told at all, as it has no compopt. Both quote the names here instead —
# unless the word already opens a quote, inside which they need none.
_zetta_tree_quote() {
    local name quote=0 index
    if [[ $(type -t compopt) != builtin ]]; then
        quote=1
    elif [[ $1 == names ]]; then
        for name in "${COMPREPLY[@]}"; do
            [[ -d $name ]] && quote=1
        done
    fi
    if (( ! quote )); then
        compopt -o filenames 2>/dev/null
        return 0
    fi
    for (( index = 0; index < ${#COMPREPLY[@]}; index++ )); do
        name=${COMPREPLY[index]}
        [[ $1 == files && -d $name ]] && name+=/
        [[ $current == [\"\']* ]] || printf -v name '%q' "$name"
        COMPREPLY[index]=$name
    done
}

# Completes the words from index $rest on as a command line of their own:
# through bash-completion's dispatcher when it is loaded, and otherwise as a
# command name and then its file arguments.
_zetta_tree_command() {
    if declare -F _command_offset >/dev/null; then
        _command_offset "$rest"
    elif (( rest == COMP_CWORD )); then
        COMPREPLY=( $(compgen -A command -- "$current") )
    else
        local line
        while IFS= read -r line; do
            COMPREPLY+=("$line")
        done < <(compgen -f -- "$current")
        _zetta_tree_quote files
    fi
}
