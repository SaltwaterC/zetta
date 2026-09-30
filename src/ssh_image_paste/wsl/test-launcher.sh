#!/bin/sh
case "${ZETTA_TEST_WSL_MODE:-}" in
    fail) exit 37 ;;
    timeout) exec sleep 2 ;;
esac
while [ "$#" -gt 0 ]; do
    case "$1" in
        --exec|-e) shift; exec "$@" ;;
        *) shift ;;
    esac
done
exit 38
