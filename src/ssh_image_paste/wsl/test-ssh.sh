#!/bin/sh
# Execute only the generated remote command, locally, without opening SSH.
while [ "$#" -gt 1 ]; do shift; done
exec /bin/sh -c "$1"
