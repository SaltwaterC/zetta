# Zetta shell integration for Fish.
if set -q ZETTA_HOST_EXECUTABLE; and test -n "$ZETTA_HOST_EXECUTABLE"
    function zetta
        command $ZETTA_HOST_EXECUTABLE $argv
    end
end

if not functions -q __zetta_report_cwd
    function __zetta_report_cwd --on-event fish_prompt
        printf '\033]2;zetta-cwd:%s\033\\' "$PWD"
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

function zwt --description 'Zetta Git worktree workflow'
    switch $argv[1]
        case new
            set -l operation_args $argv[2..-1]
            set -l path
            if contains -- --help $operation_args; or contains -- -h $operation_args
                zetta wt new $operation_args
                return $status
            else if contains -- --path-only $operation_args; or contains -- -P $operation_args
                set path (zetta wt new $operation_args)
            else
                set path (zetta wt new --path-only $operation_args)
            end
            or return
            test (count $path) -eq 1
            or return 1
            builtin cd -- $path[1]
        case done
            set -l operation_args $argv[2..-1]
            set -l path
            if contains -- --help $operation_args; or contains -- -h $operation_args
                zetta wt done $operation_args
                return $status
            else if contains -- --path-only $operation_args; or contains -- -P $operation_args
                set path (zetta wt done $operation_args)
            else
                set path (zetta wt done --path-only $operation_args)
            end
            or return
            test (count $path) -eq 1
            or return 1
            builtin cd -- $path[1]
        case '*'
            zetta wt $argv
    end
end

function ztftp --wraps 'zetta tftp' --description 'Zetta TFTP client'
    zetta tftp $argv
end

function zntfy --wraps 'zetta notify' --description 'Zetta desktop notifications'
    zetta notify $argv
end

function zcopy --wraps 'zetta copy' --description 'Copy standard input to the clipboard'
    zetta copy $argv
end

function zpaste --wraps 'zetta paste' --description "Print the clipboard's contents"
    zetta paste $argv
end

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste function or abbreviation is erased first so
# Zetta's functions take priority over it.
switch (uname)
    case Darwin
    case '*'
        functions -e pbcopy pbpaste 2>/dev/null
        function pbcopy --wraps 'zetta copy' --description 'Copy standard input to the clipboard'
            zetta copy $argv
        end
        function pbpaste --wraps 'zetta paste' --description "Print the clipboard's contents"
            zetta paste $argv
        end
end

function __zetta_config_args
    set -l words (commandline -opc)
    set -l args
    set -l index 2
    while test $index -le (count $words)
        switch $words[$index]
            case --config -c
                if test $index -lt (count $words)
                    set index (math $index + 1)
                    set args $args --config $words[$index]
                end
        end
        set index (math $index + 1)
    end
    printf '%s\n' $args
end

function __zetta_profiles
    set -l config_args (__zetta_config_args)
    zetta profile list $config_args 2>/dev/null
end

function __zetta_profile_themes
    set -l config_args (__zetta_config_args)
    zetta profile themes $config_args 2>/dev/null
end

function __zetta_serial_devices
    zetta serial list 2>/dev/null
end

function __zetta_tab_icons
    zetta tabicon --list 2>/dev/null
end

function __zetta_pane_themes
    zetta panetheme --list 2>/dev/null
end

function __zetta_pane_splits
    zetta splits 2>/dev/null
end

function __zetta_projects
    zetta project list 2>/dev/null
end

function __zetta_project_is
    set -l words (commandline -opc)
    test (count $words) -ge 3
    and test "$words[2]" = project
    and contains -- "$words[3]" $argv
end

function __zetta_pane_labels
    zetta pane --list 2>/dev/null
end

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered.
function __zetta_sound_names
    switch (uname)
        case Darwin
            printf '%s\n' zetta-default zetta-ok zetta-alarm \
                Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink
        case '*'
            printf '%s\n' zetta-default zetta-ok zetta-alarm bell complete message \
                message-new-instant dialog-information dialog-warning dialog-error trash-empty
    end
end

# Fish completion registrations normally expose every short option as a
# candidate. Keep the short forms out of the candidate list while retaining
# their argument completion by activating these registrations only after the
# user has already entered the short option.
function __zetta_short_option
    set -l words (commandline -opc)
    test (count $words) -gt 0
    and test "$words[-1]" = "$argv[1]"
end

function __zetta_at_subcommand
    set -l words (commandline -opc)
    test (count $words) -eq 2
    and test "$words[2]" = "$argv[1]"
end

function __zetta_profile_operation
    set -l words (commandline -opc)
    set -l seen 0
    set -l skip 0
    for word in $words[2..-1]
        if test $skip -eq 1
            set skip 0
            continue
        end
        if test $seen -eq 0
            switch $word
                case --config -c --keymap -k --profile -p --split -s --theme -t
                    set skip 1
                case profile
                    set seen 1
            end
            continue
        end
        if test $seen -eq 1
            switch $word
                case --config -c
                    set skip 1
                case --icon -i
                    set skip 1
                case '-*'
                case '*'
                    printf '%s\n' $word
                    return 0
            end
        end
    end
    return 1
end

function __zetta_has_profile_subcommand
    set -l words (commandline -opc)
    set -l skip 0
    for word in $words[2..-1]
        if test $skip -eq 1
            set skip 0
            continue
        end
        switch $word
            case --config -c --keymap -k --profile -p --split -s --theme -t
                set skip 1
            case profile
                return 0
        end
    end
    return 1
end

function __zetta_at_profile_command
    __zetta_has_profile_subcommand; or return 1
    set -l operation (__zetta_profile_operation)
    test (count $operation) -eq 0
end

function __zetta_profile_argument_count
    set -l words (commandline -opc)
    set -l operation (__zetta_profile_operation)
    set -l seen 0
    set -l skip 0
    set -l count 0
    for word in $words[2..-1]
        if test $skip -eq 1
            set skip 0
            continue
        end
        if test $seen -eq 0
            switch $word
                case --config -c --keymap -k --profile -p --split -s --theme -t
                    set skip 1
                case profile
                    set seen 1
            end
            continue
        end
        if test "$word" = "$operation"
        else
            switch $word
                case --config -c
                    set skip 1
                case '-*'
                case '*'
                    set count (math $count + 1)
            end
        end
    end
    printf '%s\n' $count
end

function __zetta_profile_is
    test (__zetta_profile_operation) = "$argv[1]"
end

function __zetta_profile_needs_profile
    test (__zetta_profile_argument_count) -eq 0
end

function __zetta_profile_needs_theme
    test (__zetta_profile_argument_count) -eq 1
end

function __zetta_profile_needs_icon
    test (__zetta_profile_argument_count) -eq 1
end

# A subcommand is only recognized as the very first argument, unlike root
# flags (--profile, --theme, --config, --keymap), which may combine and
# appear in any order. Subcommand-name candidates use this instead of
# __zetta_use_subcommand so they stop appearing once a root flag is typed.
function __zetta_at_root
    test (count (commandline -opc)) -eq 1
end

# Fish's own __fish_use_subcommand treats any non-flag token as a subcommand,
# so it stops offering root flags after a value-taking one (e.g. --profile
# NAME) even though no subcommand was actually given. Skip known root option
# arguments before applying that same rule, so --profile and --theme keep
# completing each other despite --theme requiring --profile.
function __zetta_use_subcommand
    set -l words (commandline -opc)
    set -e words[1]
    set -l skip_next 0
    for word in $words
        if test $skip_next -eq 1
            set skip_next 0
            continue
        end
        switch $word
            case --config -c --keymap -k --profile -p --split -s --theme -t
                set skip_next 1
                continue
            case '-*'
                continue
        end
        return 1
    end
    return 0
end

function __zetta_tftp_client
    set -l words (commandline -opc)
    test (count $words) -ge 3
    and test "$words[2]" = tftp
    and contains -- "$words[3]" get put
end

function __zetta_tftp_server
    set -l words (commandline -opc)
    test (count $words) -ge 3
    and test "$words[2]" = tftp
    and test "$words[3]" = server
end

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

function __zetta_mux_daemon_commands
    test "$ZETTA_NO_MUX" != 1
end

# Fish only considers options registered with `-l` after the user has typed a
# dash. Emit the same long options as ordinary completion candidates too, so
# they appear alongside subcommands at every valid argument position.
function __zetta_option_unused
    set -l words (commandline -opc)
    test "$argv[1]" = --copy; and return 0
    not contains -- $argv[1] $words[2..-1]
end

function __zetta_filter_long_options
    while read -l line
        set -l option (string split \t -- $line)[1]
        if __zetta_option_unused $option
            printf '%s\n' "$line"
        end
    end
end

function __zetta_long_options
    begin
        switch $argv[1]
        case root
            printf '%s\t%s\n' \
                --help 'Print help' \
                --version 'Print version' \
                --config 'Use a configuration file' \
                --keymap 'Use a keymap file' \
                --profile 'Select a profile' \
                --split 'Apply a configured pane split template' \
                --replace-pane 'Replace the active pane in a running process' \
                --theme 'Non-persistently override the profile theme' \
                --no-mux 'Keep background sessions in this process for this launch'
        case profile
            printf '%s\t%s\n' \
                list 'List all resolved profiles' \
                themes 'List available profile themes' \
                disable 'Hide a profile' \
                enable 'Show a profile' \
                theme 'Set or reset a profile theme' \
                icon 'Set or reset a profile icon' \
                default 'Set the default profile' \
                add 'Add a custom profile' \
                remove 'Remove a custom profile' \
                --config 'Use a configuration file' \
                --help 'Print help'
        case init serial http tftp splits
            printf '%s\t%s\n' --help 'Print help'
        case pane
            printf '%s\t%s\n' \
                --direction 'Direction for a new split' \
                --label 'Label for a new split pane' \
                --pane 'Target pane label' \
                --overlay 'Overlay text for a new split pane' \
                --overlay-size 'Overlay font size' \
                --overlay-opacity 'Overlay opacity percentage' \
                --overlay-color 'Overlay text color' \
                --stack 'Run in a stacked task pane' \
                --list 'List pane labels' \
                --help 'Print help'
        case panetheme
            printf '%s\t%s\n' \
                --theme 'Set the pane theme' \
                --reset 'Restore the profile-configured theme' \
                --list 'Print the registered theme names' \
                --help 'Print help'
        case tabicon
            printf '%s\t%s\n' \
                --icon 'Set the tab icon' \
                --list 'Print built-in icon names' \
                --help 'Print help'
        case overlay
            printf '%s\t%s\n' \
                --text 'Set the overlay text' \
                --size 'Set the font size' \
                --opacity 'Set the opacity percentage (0-100)' \
                --color 'Set the text color (name or hex)' \
                --reset 'Clear the overlay' \
                --help 'Print help'
        case wt
            printf '%s\t%s\n' \
                new 'Create a worktree' \
                done 'Integrate and remove the current worktree' \
                status 'Show worktree state' \
                rerere 'Enable Git rerere' \
                --help 'Print help'
        case terminal-size
            printf '%s\t%s\n' \
                --json 'Print machine-readable JSON' \
                --resize 'Resize the current pane' \
                --columns 'Set pane width in columns' \
                --rows 'Set pane height in rows' \
                --help 'Print help'
        case edit
            printf '%s\t%s\n' --delete-after 'Delete a managed buffer after editing' --help 'Print help'
        case vi
            printf '%s\t%s\n' --help 'Print help'
        case mux
            if test "$ZETTA_NO_MUX" = 1
                printf '%s\t%s\n' --json 'Print machine-readable JSON' --help 'Print help' --version 'Print version'
            else
                printf '%s\t%s\n' --json 'Print machine-readable JSON' --force 'Stop even while sessions are running' --upgrade 'Replace the multiplexer, keeping its sessions' --help 'Print help' --version 'Print version'
            end
        case benchmark-output
            printf '%s\t%s\n' \
                --size 'Set the output size in MiB' \
                --output-type 'Select repeated or unique lines' \
                --help 'Print help'
        case benchmark
            printf '%s\n' \
                --profile-report \
                --profile-duration \
                --profile-pane-stress \
                --profile-background-stress \
                --profile-sparse-updates \
                --profile-alt-screen-scroll \
                --profile-external-terminal \
                --help
        case serial-console
            printf '%s\t%s\n' \
                --device 'Serial device' \
                --baud-rate 'Baud rate' \
                --data-bits 'Data bits' \
                --parity 'Parity' \
                --stop-bits 'Stop bits' \
                --flow-control 'Flow control' \
                --help 'Print help'
        case http-server
            printf '%s\t%s\n' \
                --root 'Directory to serve' \
                --port 'Server port' \
                --config 'Configuration file' \
                --help 'Print help'
        case tftp-server
            printf '%s\t%s\n' \
                --root 'Directory to serve' \
                --port 'Server port' \
                --config 'Configuration file' \
                --writable 'Accept uploads into the served directory' \
                --help 'Print help'
        case tftp-client ztftp
            printf '%s\t%s\n' --port 'Server port' --help 'Print help'
        case notify zntfy
            printf '%s\t%s\n' \
                --app-name 'Application name' \
                --icon 'Image to show with the notification' \
                --sound 'Sound name' \
                --timeout 'Timeout' \
                --help 'Print help'
        case notify-cleanup
            printf '%s\t%s\n' \
                --dry-run 'List stale workers without terminating them' \
                --help 'Print help'
        case attention
            printf '%s\t%s\n' \
                --notify 'Also show a desktop notification' \
                --app-name 'Application name' \
                --icon 'Image to show with the notification' \
                --sound 'Sound name' \
                --timeout 'Timeout' \
                --help 'Print help'
        case copy zcopy pbcopy
            printf '%s\t%s\n' --pboard 'Pasteboard to use' --help 'Print help'
        case paste zpaste pbpaste
            printf '%s\t%s\n' \
                --pboard 'Pasteboard to use' \
                --prefer 'Preferred clipboard format' \
                --help 'Print help'
        end
    end | __zetta_filter_long_options
end

complete -c zetta -f
complete -c zetta -n '__zetta_at_root' -a benchmark -d 'Profile terminal rendering'
complete -c zetta -n '__zetta_at_root' -a benchmark-output -d 'Write and time a text payload'
complete -c zetta -n '__zetta_at_root' -a terminal-size -d 'Print the current terminal size'
complete -c zetta -n '__zetta_at_root' -a mux -d 'Control the session multiplexer'
complete -c zetta -n '__zetta_at_root' -a profile -d 'List and manage profiles'
complete -c zetta -n '__zetta_at_root' -a project -d 'List and manage projects'
complete -c zetta -n '__zetta_at_root' -a edit -d 'Edit files with EDITOR or Zetta vi'
complete -c zetta -n '__zetta_at_root' -a vi -d "Edit files with Zetta's built-in vi"
complete -c zetta -n '__zetta_at_root' -a init -d 'Generate shell integration'
complete -c zetta -n '__zetta_at_root' -a serial -d 'List or connect to serial devices'
complete -c zetta -n '__zetta_at_root' -a http -d 'Serve static files over HTTP'
complete -c zetta -n '__zetta_at_root' -a tftp -d 'Transfer a file with TFTP'
complete -c zetta -n '__zetta_at_root' -a notify -d 'Show a desktop notification'
complete -c zetta -n '__zetta_at_root' -a notify-cleanup -d 'Reap stale desktop notification worker processes'
complete -c zetta -n '__zetta_at_root' -a attention -d 'Mark the originating tab as needing attention'
complete -c zetta -n '__zetta_at_root' -a copy -d 'Copy standard input to the clipboard'
complete -c zetta -n '__zetta_at_root' -a paste -d "Print the clipboard's contents"
complete -c zetta -n '__zetta_at_root' -a tabicon -d 'Set the active tab icon'
complete -c zetta -n '__zetta_at_root' -a panetheme -d "Non-persistently change the active pane's theme"
complete -c zetta -n '__zetta_at_root' -a splits -d 'List configured pane split templates'
complete -c zetta -n '__zetta_at_root' -a pane -d 'Run a command in a pane'
complete -c zetta -n '__zetta_at_root' -a overlay -d 'Non-persistently show text over the active pane'
complete -c zetta -n '__zetta_at_root' -a wt -d 'Create and integrate Git worktrees'
complete -c zetta -n '__zetta_use_subcommand' -l help -d 'Print help'
complete -c zetta -n '__zetta_use_subcommand' -l version -d 'Print version'
complete -c zetta -n '__zetta_use_subcommand' -l config -r -d 'Use a configuration file'
complete -c zetta -n '__zetta_use_subcommand' -l keymap -r -d 'Use a keymap file'
complete -c zetta -n '__zetta_use_subcommand' -l profile -r -a '(__zetta_profiles)' -d 'Select a profile'
complete -c zetta -n '__zetta_use_subcommand' -l split -r -a '(__zetta_pane_splits)' -d 'Apply a configured pane split template'
complete -c zetta -n '__zetta_use_subcommand' -l replace-pane -d 'Replace the active pane in a running process'
complete -c zetta -n '__zetta_use_subcommand' -l theme -r -a '(__zetta_profile_themes)' -d 'Non-persistently override the profile theme'
complete -c zetta -n '__zetta_use_subcommand' -l no-mux -d 'Keep background sessions in this process for this launch'
complete -c zetta -n '__zetta_use_subcommand' -a '(__zetta_long_options root)'
complete -c zetta -s c -r -n '__zetta_use_subcommand; and __zetta_short_option -c'
complete -c zetta -s k -r -n '__zetta_use_subcommand; and __zetta_short_option -k'
complete -c zetta -s p -r -a '(__zetta_profiles)' -n '__zetta_use_subcommand; and __zetta_short_option -p'
complete -c zetta -s s -r -a '(__zetta_pane_splits)' -n '__zetta_use_subcommand; and __zetta_short_option -s'
complete -c zetta -s r -n '__zetta_use_subcommand; and __zetta_short_option -r'
complete -c zetta -s t -r -a '(__zetta_profile_themes)' -n '__zetta_use_subcommand; and __zetta_short_option -t'
complete -c zetta -n '__zetta_at_profile_command' -a '(__zetta_long_options profile)'
complete -c zetta -n '__zetta_has_profile_subcommand' -l config -r -d 'Use a configuration file'
complete -c zetta -s c -r -n '__zetta_has_profile_subcommand; and __zetta_short_option -c'
complete -c zetta -n '__zetta_has_profile_subcommand' -l help -d 'Print help'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is disable; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Profile to hide'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is enable; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Profile to show'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is default; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Default profile'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is remove; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Profile to remove'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is theme; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Profile to theme'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is theme; and __zetta_profile_needs_theme' -a '(__zetta_profile_themes)' -d 'Profile theme'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is theme' -l reset -d 'Remove the profile theme override'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is icon; and __zetta_profile_needs_profile' -a '(__zetta_profiles)' -d 'Profile to set an icon for'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is icon; and __zetta_profile_needs_icon' -a 'auto zetta bash zsh fish' -d 'Profile icon'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is icon' -l reset -d 'Restore automatic profile icon inference'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is add' -l program -r -d 'Program to launch'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is add' -l arg -r -d 'Program argument'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is add' -l theme -r -a '(__zetta_profile_themes)' -d 'Profile theme'
complete -c zetta -n '__zetta_has_profile_subcommand; and __zetta_profile_is add' -l icon -r -a 'auto zetta bash zsh fish' -d 'Profile icon override'
complete -c zetta -s p -r -n '__zetta_has_profile_subcommand; and __zetta_profile_is add; and __zetta_short_option -p'
complete -c zetta -s a -r -n '__zetta_has_profile_subcommand; and __zetta_profile_is add; and __zetta_short_option -a'
complete -c zetta -s t -r -a '(__zetta_profile_themes)' -n '__zetta_has_profile_subcommand; and __zetta_profile_is add; and __zetta_short_option -t'
complete -c zetta -s r -n '__zetta_has_profile_subcommand; and __zetta_profile_is theme; and __zetta_short_option -r'
complete -c zetta -s i -r -a 'auto zetta bash zsh fish' -n '__zetta_has_profile_subcommand; and __zetta_profile_is add; and __zetta_short_option -i'
complete -c zetta -s r -n '__zetta_has_profile_subcommand; and __zetta_profile_is icon; and __zetta_short_option -r'
complete -c zetta -n '__zetta_at_subcommand init' -a 'bash fish powershell pwsh zsh'
complete -c zetta -n '__fish_seen_subcommand_from init' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from init' -a '(__zetta_long_options init)'
complete -c zetta -n '__zetta_at_subcommand serial' -a 'console list'
complete -c zetta -n '__fish_seen_subcommand_from serial' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from serial' -a '(__zetta_long_options serial)'
complete -c zetta -n '__zetta_at_subcommand http' -a server
complete -c zetta -n '__fish_seen_subcommand_from http' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from http' -a '(__zetta_long_options http)'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__zetta_at_subcommand mux' -a list -d 'List the sessions the multiplexer is holding'
complete -c zetta -n '__zetta_at_subcommand mux; and __zetta_mux_daemon_commands' -a stop -d 'Stop the multiplexer'
complete -c zetta -n '__zetta_at_subcommand mux' -a reconnect -d 'Open a session in a Zetta window'
complete -c zetta -n '__zetta_at_subcommand mux; and __zetta_mux_daemon_commands' -a share -d 'Let every Zetta process attach a backgrounded session'
complete -c zetta -n '__zetta_at_subcommand mux; and __zetta_mux_daemon_commands' -a unshare -d 'Scope a session back to the window that held it'
complete -c zetta -n '__zetta_at_subcommand mux; and __zetta_mux_daemon_commands' -a kill -d 'End a session and everything running in it'
complete -c zetta -n '__zetta_at_subcommand mux; and __zetta_mux_daemon_commands' -a forget -d 'Remove a session from the catalog without killing it'
complete -c zetta -n '__fish_seen_subcommand_from mux; and __fish_seen_subcommand_from reconnect' -a '(__zetta_mux_session_ids)' -d 'Multiplexer session ID'
complete -c zetta -n '__fish_seen_subcommand_from mux; and __fish_seen_subcommand_from share unshare kill forget; and __zetta_mux_daemon_commands' -a '(__zetta_mux_session_ids)' -d 'Multiplexer session ID'
complete -c zetta -n '__fish_seen_subcommand_from mux; and __zetta_mux_daemon_commands' -l force -d 'Stop even while sessions are running'
complete -c zetta -n '__fish_seen_subcommand_from mux; and __zetta_mux_daemon_commands' -l upgrade -d 'Replace the multiplexer, keeping its sessions'
complete -c zetta -n '__fish_seen_subcommand_from mux' -l json -d 'Print machine-readable JSON'
complete -c zetta -n '__fish_seen_subcommand_from mux' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from mux' -a '(__zetta_long_options mux)'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l resize -d 'Resize the current pane'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l columns -r -d 'Set pane width in columns'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l rows -r -d 'Set pane height in rows'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from terminal-size' -a '(__zetta_long_options terminal-size)'
complete -c zetta -s c -r -n '__fish_seen_subcommand_from terminal-size; and __zetta_short_option -c'
complete -c zetta -s R -r -n '__fish_seen_subcommand_from terminal-size; and __zetta_short_option -R'
complete -c zetta -n '__fish_seen_subcommand_from splits' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from splits' -a '(__zetta_long_options splits)'
complete -c zetta -n '__zetta_at_subcommand project' -a 'add list remove open'
complete -c zetta -n '__fish_seen_subcommand_from project' -l help -d 'Print help'
complete -c zetta -n '__zetta_project_is add remove open' -l path -r -d 'Project root'
complete -c zetta -n '__zetta_project_is add' -a '(__fish_complete_directories)'
complete -c zetta -n '__zetta_project_is open remove' -a '(__zetta_projects)'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l direction -r -a 'left right up down' -d 'Direction for a new split'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l label -r -d 'Label for a new split pane'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l pane -r -a '(__zetta_pane_labels)' -d 'Target pane label'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l overlay -r -d 'Overlay text for a new split pane'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l overlay-size -r -a 'sm base lg xl 2xl 3xl' -d 'Overlay font size'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l overlay-opacity -r -d 'Overlay opacity percentage'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l overlay-color -r -a 'ZETTA_OVERLAY_COLORS' -d 'Overlay text color'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l stack -d 'Run in a stacked task pane'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l list -d 'List pane labels'
complete -c zetta -n '__fish_seen_subcommand_from pane' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from pane' -a '(__zetta_long_options pane)'
complete -c zetta -s d -r -a 'left right up down' -n '__fish_seen_subcommand_from pane; and __zetta_short_option -d'
complete -c zetta -s l -r -n '__fish_seen_subcommand_from pane; and __zetta_short_option -l'
complete -c zetta -s p -r -a '(__zetta_pane_labels)' -n '__fish_seen_subcommand_from pane; and __zetta_short_option -p'
complete -c zetta -s o -r -n '__fish_seen_subcommand_from pane; and __zetta_short_option -o'
complete -c zetta -s S -r -a 'sm base lg xl 2xl 3xl' -n '__fish_seen_subcommand_from pane; and __zetta_short_option -S'
complete -c zetta -s O -r -n '__fish_seen_subcommand_from pane; and __zetta_short_option -O'
complete -c zetta -s c -r -a 'ZETTA_OVERLAY_COLORS' -n '__fish_seen_subcommand_from pane; and __zetta_short_option -c'
complete -c zetta -n '__fish_seen_subcommand_from edit' -l delete-after -d 'Delete a managed buffer after editing'
complete -c zetta -n '__fish_seen_subcommand_from edit' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from edit' -a '(__zetta_long_options edit)'
complete -c zetta -n '__fish_seen_subcommand_from edit' -F
complete -c zetta -s d -n '__fish_seen_subcommand_from edit; and __zetta_short_option -d'
complete -c zetta -n '__fish_seen_subcommand_from vi' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from vi' -a '(__zetta_long_options vi)'
complete -c zetta -n '__fish_seen_subcommand_from vi' -F
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l size -r -d 'Set the output size in MiB'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l output-type -r -a 'repeated unique'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from benchmark-output' -a '(__zetta_long_options benchmark-output)'
complete -c zetta -s s -r -n '__fish_seen_subcommand_from benchmark-output; and __zetta_short_option -s'
complete -c zetta -s t -r -a 'repeated unique' -n '__fish_seen_subcommand_from benchmark-output; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-report -r
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-duration -r
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-pane-stress
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-background-stress
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-sparse-updates
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-alt-screen-scroll
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l profile-external-terminal
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from benchmark' -a '(__zetta_long_options benchmark)'
complete -c zetta -s r -r -n '__fish_seen_subcommand_from benchmark; and __zetta_short_option -r'
complete -c zetta -s d -r -n '__fish_seen_subcommand_from benchmark; and __zetta_short_option -d'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l device -r -a '(__zetta_serial_devices)' -d 'Serial device'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l baud-rate -r -d 'Baud rate'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l data-bits -r -a '5 6 7 8'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l parity -r -a 'none odd even'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l stop-bits -r -a '1 2'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -l flow-control -r -a 'none software hardware'
complete -c zetta -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console' -a '(__zetta_long_options serial-console)'
complete -c zetta -s d -r -a '(__zetta_serial_devices)' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -d'
complete -c zetta -s b -r -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -b'
complete -c zetta -s D -r -a '5 6 7 8' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -D'
complete -c zetta -s p -r -a 'none odd even' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -p'
complete -c zetta -s s -r -a '1 2' -n '__fish_seen_subcommand_from serial; and __fish_seen_subcommand_from console; and __zetta_short_option -s'
complete -c zetta -s f -r -a 'none software hardware' -n '__fish_seen_subcommand_from serial; and __zetta_short_option -f'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l port -r -d 'TCP port'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -l config -r -d 'Configuration file'
complete -c zetta -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server' -a '(__zetta_long_options http-server)'
complete -c zetta -s r -r -a '(__fish_complete_directories)' -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -r'
complete -c zetta -s p -r -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -p'
complete -c zetta -s c -r -n '__fish_seen_subcommand_from http; and __fish_seen_subcommand_from server; and __zetta_short_option -c'
complete -c zetta -n '__zetta_at_subcommand tftp' -a 'get put server'
complete -c zetta -n '__zetta_tftp_client' -l port -r -d 'Server port'
complete -c zetta -n '__zetta_tftp_server' -l root -r -a '(__fish_complete_directories)' -d 'Directory to serve'
complete -c zetta -n '__zetta_tftp_server' -l config -r -d 'Configuration file'
complete -c zetta -n '__zetta_tftp_server' -l writable -d 'Accept uploads into the served directory'
complete -c zetta -n '__fish_seen_subcommand_from tftp' -l help -d 'Print help'
complete -c zetta -n '__zetta_at_subcommand tftp' -a '(__zetta_long_options tftp)'
complete -c zetta -n '__zetta_tftp_client' -a '(__zetta_long_options tftp-client)'
complete -c zetta -n '__zetta_tftp_server' -a '(__zetta_long_options tftp-server)'
complete -c zetta -s p -r -n '__zetta_tftp_client; and __zetta_short_option -p'
complete -c zetta -s r -r -a '(__fish_complete_directories)' -n '__zetta_tftp_server; and __zetta_short_option -r'
complete -c zetta -s p -r -n '__zetta_tftp_server; and __zetta_short_option -p'
complete -c zetta -s c -r -n '__zetta_tftp_server; and __zetta_short_option -c'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l app-name -r -d 'Application name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l icon -r -d 'Image to show with the notification'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l icon -r -a '(__zetta_tab_icons)' -d 'Set the tab icon'
complete -c zetta -s i -r -a '(__zetta_tab_icons)' -n '__fish_seen_subcommand_from tabicon; and __zetta_short_option -i'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l sound -r -a '(__zetta_sound_names)' -d 'Sound name'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l timeout -r -a 'default never' -d 'Timeout'
complete -c zetta -n '__fish_seen_subcommand_from notify' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from notify' -a '(__zetta_long_options notify)'
complete -c zetta -s a -r -n '__fish_seen_subcommand_from notify; and __zetta_short_option -a'
complete -c zetta -s i -r -n '__fish_seen_subcommand_from notify; and __zetta_short_option -i'
complete -c zetta -s s -r -a '(__zetta_sound_names)' -n '__fish_seen_subcommand_from notify; and __zetta_short_option -s'
complete -c zetta -s t -r -a 'default never' -n '__fish_seen_subcommand_from notify; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from notify-cleanup' -l dry-run -d 'List stale workers without terminating them'
complete -c zetta -n '__fish_seen_subcommand_from notify-cleanup' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from notify-cleanup' -a '(__zetta_long_options notify-cleanup)'
complete -c zetta -s n -n '__fish_seen_subcommand_from notify-cleanup; and __zetta_short_option -n'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l notify -d 'Also show a desktop notification'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l app-name -r -d 'Application name'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l icon -r -d 'Image to show with the notification'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l sound -r -a '(__zetta_sound_names)' -d 'Sound name'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l timeout -r -a 'default never' -d 'Timeout'
complete -c zetta -n '__fish_seen_subcommand_from attention' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from attention' -a '(__zetta_long_options attention)'
complete -c zetta -s n -n '__fish_seen_subcommand_from attention; and __zetta_short_option -n'
complete -c zetta -s a -r -n '__fish_seen_subcommand_from attention; and __zetta_short_option -a'
complete -c zetta -s i -r -n '__fish_seen_subcommand_from attention; and __zetta_short_option -i'
complete -c zetta -s s -r -a '(__zetta_sound_names)' -n '__fish_seen_subcommand_from attention; and __zetta_short_option -s'
complete -c zetta -s t -r -a 'default never' -n '__fish_seen_subcommand_from attention; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from copy' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from copy' -a '(__zetta_long_options copy)'
complete -c zetta -n '__fish_seen_subcommand_from copy; and __zetta_short_option -pboard' -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l pboard -r -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l prefer -r -a 'txt rtf ps'
complete -c zetta -n '__fish_seen_subcommand_from paste' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from paste' -a '(__zetta_long_options paste)'
complete -c zetta -n '__fish_seen_subcommand_from paste; and __zetta_short_option -pboard' -a 'general ruler find font'
complete -c zetta -n '__fish_seen_subcommand_from paste; and __zetta_short_option -prefer' -a 'txt rtf ps'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l list -d 'Print built-in icon names'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -a '(__zetta_long_options tabicon)'
complete -c zetta -n '__fish_seen_subcommand_from tabicon' -a '(__zetta_tab_icons)'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l theme -r -a '(__zetta_pane_themes)' -d 'Set the pane theme'
complete -c zetta -s t -r -a '(__zetta_pane_themes)' -n '__fish_seen_subcommand_from panetheme; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l reset -d 'Restore the profile-configured theme'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l list -d 'Print the registered theme names'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -a '(__zetta_long_options panetheme)'
complete -c zetta -n '__fish_seen_subcommand_from panetheme' -a '(__zetta_pane_themes)'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l text -r -d 'Set the overlay text'
complete -c zetta -s t -r -n '__fish_seen_subcommand_from overlay; and __zetta_short_option -t'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l size -r -a 'sm base lg xl 2xl 3xl' -d 'Set the font size'
complete -c zetta -s s -r -a 'sm base lg xl 2xl 3xl' -n '__fish_seen_subcommand_from overlay; and __zetta_short_option -s'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l opacity -r -d 'Set the opacity percentage (0-100)'
complete -c zetta -s o -r -n '__fish_seen_subcommand_from overlay; and __zetta_short_option -o'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l color -r -a 'ZETTA_OVERLAY_COLORS' -d 'Set the text color (name or hex)'
complete -c zetta -s c -r -a 'ZETTA_OVERLAY_COLORS' -n '__fish_seen_subcommand_from overlay; and __zetta_short_option -c'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l reset -d 'Clear the overlay'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from overlay' -a '(__zetta_long_options overlay)'
complete -c zetta -n '__zetta_at_subcommand wt' -a 'new done status rerere'
complete -c zetta -n '__fish_seen_subcommand_from wt' -l help -d 'Print help'
complete -c zetta -n '__fish_seen_subcommand_from wt' -a '(__zetta_long_options wt)'
complete -c zetta -n '__fish_seen_subcommand_from wt; and __fish_seen_subcommand_from new done' -l path-only -d 'Print only the resulting path'
complete -c zetta -n '__fish_seen_subcommand_from wt; and __fish_seen_subcommand_from new' -l copy -r -F -d 'Copy a source-worktree path (repeatable)'
complete -c zetta -s c -r -F -n '__fish_seen_subcommand_from wt; and __fish_seen_subcommand_from new; and __zetta_short_option -c'
complete -c zwt -f
complete -c zwt -n '__fish_use_subcommand' -a 'new done status rerere'
complete -c zwt -n '__fish_seen_subcommand_from new done' -l path-only -d 'Print only the resulting path'
complete -c zwt -n '__fish_seen_subcommand_from new' -l copy -r -F -d 'Copy a source-worktree path (repeatable)'
complete -c zwt -s c -r -F -n '__fish_seen_subcommand_from new; and __zetta_short_option -c'
complete -c zwt -n '__fish_seen_subcommand_from new done status rerere' -l help -d 'Print help'
complete -c zmux -f
complete -c zmux -n '__fish_use_subcommand' -a list -d 'List the sessions the multiplexer is holding'
complete -c zmux -n '__fish_use_subcommand; and __zetta_mux_daemon_commands' -a stop -d 'Stop the multiplexer'
complete -c zmux -n '__fish_use_subcommand' -a reconnect -d 'Open a session in a Zetta window'
complete -c zmux -n '__fish_use_subcommand; and __zetta_mux_daemon_commands' -a share -d 'Let every Zetta process attach a backgrounded session'
complete -c zmux -n '__fish_use_subcommand; and __zetta_mux_daemon_commands' -a unshare -d 'Scope a session back to the window that held it'
complete -c zmux -n '__fish_use_subcommand; and __zetta_mux_daemon_commands' -a kill -d 'End a session and everything running in it'
complete -c zmux -n '__fish_use_subcommand; and __zetta_mux_daemon_commands' -a forget -d 'Remove a session from the catalog without killing it'
complete -c zmux -n '__fish_seen_subcommand_from reconnect' -a '(__zetta_mux_session_ids)' -d 'Multiplexer session ID'
complete -c zmux -n '__fish_seen_subcommand_from share unshare kill forget; and __zetta_mux_daemon_commands' -a '(__zetta_mux_session_ids)' -d 'Multiplexer session ID'
complete -c zmux -n '__zetta_mux_daemon_commands' -l force -d 'Stop even while sessions are running'
complete -c zmux -n '__zetta_mux_daemon_commands' -l upgrade -d 'Replace the multiplexer, keeping its sessions'
complete -c zmux -l json -d 'Print machine-readable JSON'
complete -c zmux -l help -d 'Print help'
complete -c zmux -l version -d 'Print version'
complete -c zmux -a '(__zetta_long_options mux)'
complete -c ztftp -f -a 'get put'
complete -c ztftp -l port -r -d 'Server port'
complete -c ztftp -l help -d 'Print help'
complete -c ztftp -a '(__zetta_long_options ztftp)'
complete -c ztftp -s p -r -n '__zetta_short_option -p'
complete -c zntfy -f -l app-name -r
complete -c zntfy -l icon -r
complete -c zntfy -l sound -r -a '(__zetta_sound_names)'
complete -c zntfy -l timeout -r -a 'default never'
complete -c zntfy -l help -d 'Print help'
complete -c zntfy -a '(__zetta_long_options zntfy)'
complete -c zntfy -s a -r -n '__zetta_short_option -a'
complete -c zntfy -s i -r -n '__zetta_short_option -i'
complete -c zntfy -s s -r -a '(__zetta_sound_names)' -n '__zetta_short_option -s'
complete -c zntfy -s t -r -a 'default never' -n '__zetta_short_option -t'
complete -c zcopy -f -l pboard -r -a 'general ruler find font'
complete -c zcopy -l help -d 'Print help'
complete -c zcopy -a '(__zetta_long_options zcopy)'
complete -c zcopy -n '__zetta_short_option -pboard' -a 'general ruler find font'
complete -c zpaste -f -l pboard -r -a 'general ruler find font'
complete -c zpaste -l prefer -r -a 'txt rtf ps'
complete -c zpaste -l help -d 'Print help'
complete -c zpaste -a '(__zetta_long_options zpaste)'
complete -c zpaste -n '__zetta_short_option -pboard' -a 'general ruler find font'
complete -c zpaste -n '__zetta_short_option -prefer' -a 'txt rtf ps'
if test (uname) != Darwin
    complete -c pbcopy -f -l pboard -r -a 'general ruler find font'
    complete -c pbcopy -l help -d 'Print help'
    complete -c pbcopy -a '(__zetta_long_options pbcopy)'
    complete -c pbcopy -n '__zetta_short_option -pboard' -a 'general ruler find font'
    complete -c pbpaste -f -l pboard -r -a 'general ruler find font'
    complete -c pbpaste -l prefer -r -a 'txt rtf ps'
    complete -c pbpaste -l help -d 'Print help'
    complete -c pbpaste -a '(__zetta_long_options pbpaste)'
    complete -c pbpaste -n '__zetta_short_option -pboard' -a 'general ruler find font'
    complete -c pbpaste -n '__zetta_short_option -prefer' -a 'txt rtf ps'
end
