#!/bin/sh

state=${1:-}

warn() {
    printf '%s\n' "warning: codex tab icon hook: $*" >&2
}

case "$state" in
    ""|idle|prompt|reset)
        ;;
    *)
        warn "unknown hook state: $state"
        exit 0
        ;;
esac

# UserPromptSubmit does not expose collaboration mode. Use the transcript as
# a best-effort bridge, and only trust a plan record for the current turn.
transcript_has_plan_with_jq() {
    transcript_path=$1
    turn_id=$2

    if [ -z "$transcript_path" ] || [ -z "$turn_id" ] ||
        [ ! -f "$transcript_path" ] || [ ! -r "$transcript_path" ]; then
        return 1
    fi

    jq -R -r --arg turn_id "$turn_id" '
        fromjson?
        | if type != "object" then
            empty
          elif (.payload? | type) != "object" then
            empty
          elif .payload.type == "task_started"
            and .payload.turn_id == $turn_id
            and .payload.collaboration_mode_kind == "plan" then
            "true"
          else
            empty
          end
    ' "$transcript_path" 2>/dev/null | grep -Fqx true
}

prompt_icon_with_jq() {
    payload_type=$(printf '%s' "$payload" | jq -r 'type') || return 1
    if [ "$payload_type" != object ]; then
        printf '%s\n' "ai_open_ai_compat"
        return 0
    fi

    if printf '%s' "$payload" | jq -e '.permission_mode == "plan"' >/dev/null 2>&1; then
        printf '%s\n' "ai_open_ai_gpt_sub"
        return 0
    fi

    transcript_path=$(printf '%s' "$payload" | jq -r '
        if type == "object" then
            (.transcript_path // "") | tostring
        else
            error("hook input must be an object")
        end
    ') || return 1
    turn_id=$(printf '%s' "$payload" | jq -r '
        (.turn_id // "") | tostring
    ') || return 1

    if transcript_has_plan_with_jq "$transcript_path" "$turn_id"; then
        printf '%s\n' "ai_open_ai_gpt_sub"
    else
        printf '%s\n' "ai_open_ai_compat"
    fi
}

icon_with_jq() {
    jq -r --arg state "$state" '
        if $state == "idle" then
            "ai_open_ai"
        elif type != "object" then
            "ai_open_ai_compat"
        elif $state == "prompt" then
            if .permission_mode == "plan" then
                "ai_open_ai_gpt_sub"
            else
                "ai_open_ai_compat"
            end
        elif .hook_event_name == "SessionStart"
            or .hook_event_name == "Stop"
            or .hook_event_name == "Interrupt"
            or .source == "startup"
            or .source == "resume"
            or .source == "clear" then
            "ai_open_ai"
        elif .permission_mode == "plan" then
            "ai_open_ai_gpt_sub"
        else
            "ai_open_ai_compat"
        end
    '
}

icon_with_python() {
    python3 -c '
import json
import sys

state = sys.argv[1]


def transcript_has_plan(payload):
    transcript_path = payload.get("transcript_path")
    turn_id = payload.get("turn_id")
    if not isinstance(transcript_path, str) or not transcript_path:
        return False
    if not isinstance(turn_id, str) or not turn_id:
        return False

    try:
        with open(transcript_path, encoding="utf-8", errors="replace") as transcript:
            for line in transcript:
                try:
                    entry = json.loads(line)
                except (TypeError, ValueError):
                    continue
                if not isinstance(entry, dict):
                    continue
                task = entry.get("payload")
                if (
                    isinstance(task, dict)
                    and task.get("type") == "task_started"
                    and task.get("turn_id") == turn_id
                    and task.get("collaboration_mode_kind") == "plan"
                ):
                    return True
    except (OSError, UnicodeError):
        return False
    return False


if state == "idle":
    print("ai_open_ai")
    sys.exit(0)

payload = json.load(sys.stdin)
if not isinstance(payload, dict):
    print("ai_open_ai_compat")
elif state == "prompt":
    if payload.get("permission_mode") == "plan" or transcript_has_plan(payload):
        print("ai_open_ai_gpt_sub")
    else:
        print("ai_open_ai_compat")
elif payload.get("hook_event_name") in {"SessionStart", "Stop", "Interrupt"}:
    print("ai_open_ai")
elif payload.get("source") in {"startup", "resume", "clear"}:
    print("ai_open_ai")
elif payload.get("permission_mode") == "plan":
    print("ai_open_ai_gpt_sub")
else:
    print("ai_open_ai_compat")
' "$state"
}

# Lifecycle calls pass their state explicitly, so they can update the tab
# without waiting for the hook payload to arrive or be parsed.
if [ "$state" = idle ]; then
    icon=ai_open_ai
elif [ "$state" = reset ]; then
    :
else
    payload=$(cat) || {
        warn "could not read the hook input"
        exit 0
    }

    if command -v jq >/dev/null 2>&1; then
        if [ "$state" = prompt ]; then
            if ! icon=$(prompt_icon_with_jq); then
                warn "could not parse the hook input as JSON"
                exit 0
            fi
        elif ! icon=$(printf '%s' "$payload" | icon_with_jq); then
            warn "could not parse the hook input as JSON"
            exit 0
        fi
    elif command -v python3 >/dev/null 2>&1; then
        if ! icon=$(printf '%s' "$payload" | icon_with_python); then
            warn "could not parse the hook input as JSON"
            exit 0
        fi
    else
        warn "neither jq nor python3 is available to parse the hook input"
        exit 0
    fi
fi

if [ "$state" != reset ]; then
    case "$icon" in
        ai_open_ai|ai_open_ai_gpt_sub|ai_open_ai_compat)
            ;;
        *)
            warn "hook input produced an unknown tab icon"
            exit 0
            ;;
    esac
fi

if [ -n "${ZETTA_HOST_EXECUTABLE:-}" ] && [ -x "$ZETTA_HOST_EXECUTABLE" ]; then
    zetta_command=$ZETTA_HOST_EXECUTABLE
elif command -v zetta >/dev/null 2>&1; then
    zetta_command=zetta
else
    warn "could not find zetta on PATH"
    exit 0
fi

attempt=1
invoke_tabicon() {
    if [ "$state" = reset ]; then
        "$zetta_command" tabicon --reset
    else
        "$zetta_command" tabicon "$icon"
    fi
}

while [ "$attempt" -le 5 ]; do
    if invoke_tabicon >/dev/null 2>&1; then
        exit 0
    fi
    if [ "$attempt" -lt 5 ]; then
        sleep 0.1
    fi
    attempt=$((attempt + 1))
done

warn "could not update the Zetta tab icon"

exit 0
