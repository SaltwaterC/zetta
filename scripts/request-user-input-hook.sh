#!/bin/sh

warn() {
    printf '%s\n' "warning: request-user-input hook: $*" >&2
}

format_with_jq() {
    jq -r '
        def display_text($fallback):
            if . == null then
                $fallback
            elif (type == "string") then
                if length == 0 then $fallback else . end
            else
                tostring
            end;

        def field($question; $name; $fallback):
            ($question | if type == "object" then .[$name] else null end)
            | display_text($fallback);

        def option_label:
            if type == "object" then
                (.label // null) | display_text("(no label)")
            elif type == "string" then
                display_text("(no label)")
            else
                "(no label)"
            end;

        def options($question):
            ($question | if type == "object" then (.options // []) else [] end) as $values
            | if ($values | type) == "array" and ($values | length) > 0 then
                $values | map("  - " + option_label) | join("\n")
              else
                "  (no options)"
              end;

        def question_block($question; $index):
            "Question " + (($index + 1) | tostring) + "\n"
            + "Header: " + field($question; "header"; "(no header)") + "\n"
            + "Question: " + field($question; "question"; "(no question text)") + "\n"
            + "Options:\n" + options($question);

        .tool_input? as $tool_input
        | ($tool_input | if type == "object" then .questions else null end) as $questions
        | if ($questions | type) == "array" and ($questions | length) > 0 then
            $questions | to_entries | map(question_block(.value; .key)) | join("\n\n")
          else
            "No question details were provided."
          end
    '
}

format_with_python() {
    python3 -c '
import json
import sys


def display_text(value, fallback):
    if value is None:
        return fallback
    if isinstance(value, str):
        return value if value else fallback
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def option_label(value):
    if isinstance(value, dict):
        return display_text(value.get("label"), "(no label)")
    if isinstance(value, str):
        return display_text(value, "(no label)")
    return "(no label)"


payload = json.load(sys.stdin)
tool_input = payload.get("tool_input") if isinstance(payload, dict) else None
questions = tool_input.get("questions") if isinstance(tool_input, dict) else None

if not isinstance(questions, list) or not questions:
    print("No question details were provided.")
else:
    blocks = []
    for index, question in enumerate(questions, start=1):
        if not isinstance(question, dict):
            question = {}
        options = question.get("options")
        if not isinstance(options, list) or not options:
            formatted_options = "  (no options)"
        else:
            formatted_options = "\n".join(f"  - {option_label(option)}" for option in options)
        header = display_text(question.get("header"), "(no header)")
        question_text = display_text(question.get("question"), "(no question text)")
        blocks.append(
            f"Question {index}\n"
            f"Header: {header}\n"
            f"Question: {question_text}\n"
            f"Options:\n{formatted_options}"
        )
    print("\n\n".join(blocks))
'
}

payload=$(cat) || {
    warn "could not read the hook input"
    exit 0
}

if command -v jq >/dev/null 2>&1; then
    if ! body=$(printf '%s' "$payload" | format_with_jq); then
        warn "could not parse the hook input with jq"
        exit 0
    fi
elif command -v python3 >/dev/null 2>&1; then
    if ! body=$(printf '%s' "$payload" | format_with_python); then
        warn "could not parse the hook input with python3"
        exit 0
    fi
else
    warn "neither jq nor python3 is available to parse the hook input"
    exit 0
fi

if ! command -v zetta >/dev/null 2>&1; then
    warn "could not find zetta on PATH"
    exit 0
fi

if ! zetta attention --notify --sound zetta-alarm "Codex input required" "$body" >&2; then
    warn "could not show Zetta desktop notification"
fi

exit 0
