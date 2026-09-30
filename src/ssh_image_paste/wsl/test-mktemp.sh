#!/bin/sh
mkdir -m 700 -- "$ZETTA_TEST_DIRECTORY"
printf 'created\n' > "$ZETTA_TEST_DIRECTORY.created"
printf '%s\n' "$ZETTA_TEST_DIRECTORY"
