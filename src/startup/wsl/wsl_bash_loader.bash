# The hook the distribution's environment held, which cwd_tracker.sh set
# aside before handing PROMPT_COMMAND to this script.
__zetta_user_prompt_command=${ZETTA_ORIGINAL_PROMPT_COMMAND-}
unset ZETTA_ORIGINAL_PROMPT_COMMAND
# Zetta's commands and completions, from the Windows executable, unless an rc
# file loaded them already. This script reports the prompt and the commands
# itself, so the integration's own trackers are marked installed: otherwise
# they add a second cwd hook to PROMPT_COMMAND and take over the DEBUG trap,
# whose hook does not mark the title with the running command. No stdin: a
# Windows program run through WSL interop reads the terminal it is given, and
# swallowed whatever was typed ahead of the first prompt.
if [[ -n ${ZETTA_HOST_EXECUTABLE:-} ]] && ! declare -F __zetta_lazy_complete >/dev/null; then
    __ZETTA_CWD_TRACKING_INSTALLED=1
    __ZETTA_LIFECYCLE_TRACKING_INSTALLED=1
    __ZETTA_LIFECYCLE_TRACKING_ENABLED=1
    eval "$("$ZETTA_HOST_EXECUTABLE" init bash </dev/null)"
fi
