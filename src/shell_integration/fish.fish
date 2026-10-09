# Zetta shell integration for Fish.
if not set -q __zetta_platform
    set -g __zetta_platform (uname)
end
if not functions -q __zetta_capture_startup_history
    function __zetta_capture_startup_history --on-event fish_postexec
        if string match -q -- '*__zed_init_command_history_*' "$argv[1]"
            set -g __zetta_startup_history_command "$argv[1]"
            commandline -f repaint
        end
    end
end
if not functions -q __zetta_remove_startup_history
    function __zetta_remove_startup_history --on-event fish_prompt
        if set -q __zetta_startup_history_command
            builtin history delete --case-sensitive --exact -- "$__zetta_startup_history_command" >/dev/null 2>/dev/null
            builtin history save
            set -e __zetta_startup_history_command
        end
    end
end

function __zetta_run_path
    command zetta $argv
end

function __zetta_run_owner
    if set -q ZETTA_HOST_EXECUTABLE
        if test -f "$ZETTA_HOST_EXECUTABLE"; and test -x "$ZETTA_HOST_EXECUTABLE"
            command "$ZETTA_HOST_EXECUTABLE" $argv
            return
        end
    end
    command zetta $argv
end

function __zetta_profile_uses_owner
    set -l index 1
    while test $index -le (count $argv)
        set -l argument $argv[$index]
        switch $argument
            case --config -c
                set index (math $index + 2)
            case profile
                set index (math $index + 1)
                while test $index -le (count $argv)
                    set argument $argv[$index]
                    switch $argument
                        case --config -c
                            set index (math $index + 2)
                        case --help -h list themes
                            return 1
                        case '*'
                            return 0
                    end
                end
                return 1
            case '*'
                return 1
        end
    end
    return 1
end

function __zetta_should_use_owner
    if test (count $argv) -eq 0
        return 0
    end

    switch $argv[1]
        case pane attention notify theme overlay
            return 0
        case cmd
            if test (count $argv) -lt 2
                return 1
            end
            switch $argv[2]
                case --list -l --help -h
                    return 1
            end
            set -l index 3
            while test $index -le (count $argv)
                switch $argv[$index]
                    case --
                        return 0
                    case --help -h
                        return 1
                end
                set index (math $index + 1)
            end
            return 0
        case project
            switch $argv[2]
                case add remove open
                    return 0
            end
            return 1
        case tabicon
            if test (count $argv) -lt 2
                return 1
            end
            switch $argv[2]
                case --list -l --help -h
                    return 1
                case '*'
                    return 0
            end
            return 1
        case profile
            __zetta_profile_uses_owner $argv
            return $status
    end

    set -l index 1
    while test $index -le (count $argv)
        set -l argument $argv[$index]
        switch $argument
            case --config -c --keymap -k --profile -p --split -s --theme -t --geometry -g --zetta-profile-actions-generation
                set index (math $index + 2)
            case --new-window --command --replace-pane
                return 0
            case profile
                __zetta_profile_uses_owner $argv
                return $status
            case '*'
                if test "$argument" = '-w'; or test "$argument" = '-e'; or test "$argument" = '-r'
                    return 0
                end
                return 1
        end
    end
    return 1
end

if set -q ZETTA_HOST_EXECUTABLE; and test -n "$ZETTA_HOST_EXECUTABLE"
    function zetta
        if __zetta_should_use_owner $argv
            __zetta_run_owner $argv
        else
            __zetta_run_path $argv
        end
    end
end

if not type -q mosh
    function mosh --wraps 'zetta mosh'
        command zetta mosh $argv
    end
    set -g __ZETTA_MOSH_WRAPPER 1
end

if not functions -q __zetta_report_cwd
    set -g __ZETTA_LIFECYCLE_TRACKING_INSTALLED 1
    if set -q ZETTA_PANE_ROUTING_ID
        set -g __ZETTA_LIFECYCLE_TRACKING_ENABLED 1
    else if set -q ZETTA_PANE_ID
        set -g __ZETTA_LIFECYCLE_TRACKING_ENABLED 1
    else
        set -g __ZETTA_LIFECYCLE_TRACKING_ENABLED 0
    end
    set -g __ZETTA_COMMAND_STARTED 0
    function __zetta_report_tracking_ready
        test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; or return
        printf '\033]2;zetta-event:tracking-ready\033\\'
    end
    function __zetta_report_preexec --on-event fish_preexec
        test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; or return
        set -g __ZETTA_COMMAND_STARTED 1
        printf '\033]2;zetta-event:command-started:%s\033\\' "$argv[1]"
    end
    function __zetta_report_cwd --on-event fish_prompt
        set -l command_status $status
        if test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; and test "$__ZETTA_COMMAND_STARTED" = 1
            printf '\033]2;zetta-event:command-finished:%s\033\\' "$command_status"
            set -g __ZETTA_COMMAND_STARTED 0
        end
        printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
    end
    __zetta_report_tracking_ready
end

# Upgrade a shell that loaded an older CWD-only integration. Removing the old
# event functions also removes their event subscriptions before the upgraded
# definitions are installed.
set -l __zetta_lifecycle_needs_install 0
if not set -q __ZETTA_LIFECYCLE_TRACKING_INSTALLED
    set -g __ZETTA_LIFECYCLE_TRACKING_INSTALLED 1
    set __zetta_lifecycle_needs_install 1
else if set -q ZETTA_PANE_ROUTING_ID; or set -q ZETTA_PANE_ID
    if test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" != 1
        set __zetta_lifecycle_needs_install 1
    end
end
if test "$__zetta_lifecycle_needs_install" = 1
    if set -q ZETTA_PANE_ROUTING_ID; or set -q ZETTA_PANE_ID
        set -g __ZETTA_LIFECYCLE_TRACKING_ENABLED 1
        set -g __ZETTA_COMMAND_STARTED 0
        functions -e __zetta_report_cwd 2>/dev/null
        functions -e __zetta_report_preexec 2>/dev/null
        function __zetta_report_tracking_ready
            test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; or return
            printf '\033]2;zetta-event:tracking-ready\033\\'
        end
        function __zetta_report_preexec --on-event fish_preexec
            test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; or return
            set -g __ZETTA_COMMAND_STARTED 1
            printf '\033]2;zetta-event:command-started:%s\033\\' "$argv[1]"
        end
        function __zetta_report_cwd --on-event fish_prompt
            set -l command_status $status
            if test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" = 1; and test "$__ZETTA_COMMAND_STARTED" = 1
                printf '\033]2;zetta-event:command-finished:%s\033\\' "$command_status"
                set -g __ZETTA_COMMAND_STARTED 0
            end
            printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
        end
        __zetta_report_tracking_ready
    else
        set -g __ZETTA_LIFECYCLE_TRACKING_ENABLED 0
    end
end

if not set -q EDITOR
    set -gx EDITOR 'zetta vi'
end

if not type -q vi
    if not abbr --query vi
        function vi --wraps 'zetta vi' --description 'Zetta vi editor'
            zetta vi $argv
        end
        complete -c vi -F
    end
end

function zvi --wraps 'zetta vi' --description 'Zetta vi editor'
    zetta vi $argv
end
complete -c zvi -F

# ZETTA_WORKTREE_INTEGRATION_BEGIN
function __zetta_worktree_commits
    set -l words (commandline -opc)
    set -l operation_index 3
    if test "$words[1]" = zwt
        set operation_index 2
    end
    set -l current_branch (git branch --show-current 2>/dev/null)
    test (count $current_branch) -eq 1
    or return
    set current_branch $current_branch[1]
    set -l source_branch (git config --local --get "wtbranch.$current_branch.base" 2>/dev/null)
    test (count $source_branch) -eq 1
    or return
    set source_branch $source_branch[1]
    set -l split_point (git merge-base "refs/heads/$current_branch" "refs/heads/$source_branch" 2>/dev/null)
    test (count $split_point) -eq 1
    or return
    set split_point $split_point[1]
    git rev-list --reverse "$split_point..refs/heads/$source_branch" 2>/dev/null
end

function __zetta_worktree_sync_target
    set -l words (commandline -opc)
    set -l current (commandline -ct)
    string match -q -- '-*' "$current"
    and return 1
    set -l operation_index 3
    if test "$words[1]" = zwt
        set operation_index 2
    end
    test "$words[$operation_index]" = sync
    or return 1
    set -l target_count 0
    set -l argument_index (math $operation_index + 1)
    while test $argument_index -le (count $words)
        if string match -q -- '-*' "$words[$argument_index]"
            return 1
        end
        set target_count (math $target_count + 1)
        set argument_index (math $argument_index + 1)
    end
    test $target_count -eq 0
end

function zwt --description 'Zetta Git worktree workflow'
    switch $argv[1]
        case new
            set -l operation_args $argv[2..-1]
            set -l path
            if contains -- --help $operation_args; or contains -- -h $operation_args
                command zwt new $operation_args
                return $status
            else if contains -- --path-only $operation_args; or contains -- -P $operation_args
                set path (command zwt new $operation_args)
            else
                set path (command zwt new --path-only $operation_args)
            end
            or return
            test (count $path) -eq 1
            or return 1
            builtin cd -- $path[1]
        case done
            set -l operation_args $argv[2..-1]
            set -l path
            if contains -- --help $operation_args; or contains -- -h $operation_args
                command zwt done $operation_args
                return $status
            else if contains -- --path-only $operation_args; or contains -- -P $operation_args
                set path (command zwt done $operation_args)
            else
                set path (command zwt done --path-only $operation_args)
            end
            or return
            test (count $path) -eq 1
            or return 1
            builtin cd -- $path[1]
        case abort
            set -l operation_args $argv[2..-1]
            set -l path
            if contains -- --help $operation_args; or contains -- -h $operation_args
                command zwt abort $operation_args
                return $status
            else if contains -- --path-only $operation_args; or contains -- -P $operation_args
                set path (command zwt abort $operation_args)
            else
                set path (command zwt abort --path-only $operation_args)
            end
            or return
            test (count $path) -eq 1
            or return 1
            builtin cd -- $path[1]
        case '*'
            command zwt $argv
    end
end
# ZETTA_WORKTREE_INTEGRATION_END

function ztftp --wraps 'zetta tftp' --description 'Zetta TFTP client'
    zetta tftp $argv
end

function zntfy --wraps 'zetta notify' --description 'Zetta desktop notifications'
    zetta notify $argv
end

# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
functions -e zcopy zpaste 2>/dev/null
# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste function or abbreviation is erased first so
# Zetta's functions take priority over it.
switch $__zetta_platform
    case Darwin
    case '*'
        functions -e pbcopy pbpaste 2>/dev/null
        function pbcopy --wraps 'zcopy' --description 'Copy standard input to the clipboard'
            command zcopy $argv
        end
        function pbpaste --wraps 'zpaste' --description "Print the clipboard's contents"
            command zpaste $argv
        end
end
# ZETTA_CLIPBOARD_INTEGRATION_END


# Fish erases argument-only registrations by command. Preserve every other entry.
function __zetta_remove_lazy_registration
    set -l registrations (complete -c $argv[1] | string match -v '*__zetta_lazy_complete*')
    complete -c $argv[1] -e
    for registration in $registrations
        eval $registration
    end
end
function __zetta_lazy_complete
    set -q __zetta_completions_loading; and return
    set -g __zetta_completions_loading 1
    set -l payload (zetta init fish --completions | string collect; test $pipestatus[1] = 0)
    set -l generated $status
    if test "$generated" = 0; and test -n "$payload"
        # Remove the loader before source installs the large registration table.
        for name in $__zetta_completion_commands
            __zetta_remove_lazy_registration $name
        end
        if printf '%s\n' "$payload" | source
            set -g __zetta_completions_loaded 1
        else
            for name in $__zetta_completion_commands
                complete -c $name -a '(__zetta_lazy_complete)'
            end
        end
    end
    set -e __zetta_completions_loading
    set -q __zetta_completions_loaded; or return
    complete -C (commandline -cp)
end
if not set -q __zetta_completions_loaded
    set -g __zetta_completion_commands zetta zosh zvi ztftp zntfy
# ZETTA_WORKTREE_INTEGRATION_BEGIN
    set -a __zetta_completion_commands zwt
# ZETTA_WORKTREE_INTEGRATION_END
# ZETTA_ZMUX_INTEGRATION_BEGIN
    set -a __zetta_completion_commands zmux
# ZETTA_ZMUX_INTEGRATION_END
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
    set -a __zetta_completion_commands zcopy zpaste
    if test "$__zetta_platform" != Darwin
        set -a __zetta_completion_commands pbcopy pbpaste
    end
# ZETTA_CLIPBOARD_INTEGRATION_END
    if set -q __ZETTA_MOSH_WRAPPER
        set -a __zetta_completion_commands mosh
    end
    for name in $__zetta_completion_commands
        __zetta_remove_lazy_registration $name
        complete -c $name -a '(__zetta_lazy_complete)'
    end
end
