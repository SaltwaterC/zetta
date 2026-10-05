#!/bin/sh

if [ "$1" = "-G" ]; then
    printf '%s\n' 'identityagent SSH_AUTH_SOCK' 'forwardagent no'
    exit 0
fi

if [ "$SSH_AUTH_SOCK" != "$ZOSH_BOOTSTRAP_LOGIN_AGENT" ]; then
    printf '%s\n' 'The login agent environment was replaced by the forwarding relay' >&2
    exit 255
fi

if [ "$1" != "-o" ]; then
    printf '%s\n' 'No explicit forwarding relay was selected' >&2
    exit 255
fi
case "$2" in
    ForwardAgent=*) relay=${2#ForwardAgent=} ;;
    *) exit 255 ;;
esac
if [ "$relay" = "$SSH_AUTH_SOCK" ] || [ ! -S "$relay" ]; then
    printf '%s\n' 'ForwardAgent must select a separate, live capture relay' >&2
    exit 255
fi

printf '%s\n' 'MOSH IP 127.0.0.1' 'MOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA'
