let zetta_marker_nonce = ($env.__ZETTA_COMMAND_MARKER_NONCE? | default '')
hide-env -i __ZETTA_COMMAND_MARKER_NONCE
let zetta_user_config = ($nu.default-config-dir | path join 'config.nu')
if ($zetta_user_config | path exists) { source $zetta_user_config }
$env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append {||
    if ($env.ZETTA_COMMAND_STARTED? | default false) {
        let status = ($env.LAST_EXIT_CODE? | default 0)
        print -n $"\e]2;zetta-event;($zetta_marker_nonce):command-finished:($status)\e\\"
        $env.ZETTA_COMMAND_STARTED = false
    }
    print -n $"\e]7;file://localhost($env.PWD)\e\\"
    if ($zetta_marker_nonce | is-empty) {
        print -n $"\e]2;zetta-cwd:($env.PWD)\e\\"
    } else {
        print -n $"\e]2;zetta-cwd;($zetta_marker_nonce):($env.PWD)\e\\"
    }
    print -n $"\e]2;zetta-cmd;($zetta_marker_nonce):nu\e\\"
})
$env.config.hooks.pre_execution = ($env.config.hooks.pre_execution | append {||
    let command = (commandline)
    $env.ZETTA_COMMAND_STARTED = true
    print -n $"\e]2;zetta-event;($zetta_marker_nonce):command-started:($command)\e\\"
    print -n $"\e]2;zetta-cmd;($zetta_marker_nonce):($command)\e\\"
})
print -n $"\e]2;zetta-event;($zetta_marker_nonce):tracking-ready\e\\"
^rm -f -- '__ZETTA_CONFIG_PATH__'
