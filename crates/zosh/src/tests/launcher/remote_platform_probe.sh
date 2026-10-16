#!/bin/sh

platform=$1
shift
last=''
for argument in "$@"; do
    # The probe must work even when Zosh's address-discovery proxy cannot.
    case "$argument" in ProxyCommand=*) exit 99;; esac
    last=$argument
done
test "$last" = 'echo ZOSH_OS_A%OS% ZOSH_OS_B$env:OS' || exit 98
if test "$platform" = windows; then
    echo 'ZOSH_OS_A%OS% ZOSH_OS_BWindows_NT'
else
    echo 'ZOSH_OS_A%OS% ZOSH_OS_B:OS'
fi
