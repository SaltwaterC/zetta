#!/bin/sh

notify() {
    sound=$1
    summary=$2
    body=$3

    if ! zetta attention --notify --sound "$sound" "$summary" "$body" >&2; then
        printf '%s\n' "warning: could not show Zetta desktop notification" >&2
    fi
}

make lint >&2
lint_status=$?
if [ "$lint_status" -ne 0 ]; then
    notify zetta-alarm "Zetta lint failed" "The stop-hook lint step failed."
    exit "$lint_status"
fi

make test >&2
test_status=$?
if [ "$test_status" -ne 0 ]; then
    notify zetta-alarm "Zetta tests failed" "The stop-hook test step failed."
    exit "$test_status"
fi

make build >&2
build_status=$?
if [ "$build_status" -ne 0 ]; then
    notify zetta-alarm "Zetta build failed" "Tests passed, but the stop-hook build step failed."
    exit "$build_status"
fi

notify zetta-ok "Zetta checks succeeded" "Tests and the development build completed successfully."
printf '%s\n' '{"continue":true}'
