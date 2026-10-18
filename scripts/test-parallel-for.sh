#!/bin/sh
# Exercise Make's bounded worker pool without compiling or spawning terminals.
set -eu
cd "$(dirname "$0")/.."
export LC_ALL=C
temporary_root=$(cd "${TMPDIR:-/tmp}" && pwd -P)
temporary=$(mktemp -d "$temporary_root/zetta-parallel.XXXXXX")
temporary=$(cd "$temporary" && pwd -P)
case "$temporary" in
    "$temporary_root"/zetta-parallel.*) trap 'rm -rf "$temporary"' EXIT ;;
    *) echo 'Unexpected temporary test directory' >&2; exit 1 ;;
esac
export PARALLEL_TEST_LOG="$temporary/events"
test_shell=$(command -v sh)
if [ "${OS:-}" = Windows_NT ]; then
    test_shell=$(cd "$(dirname "$test_shell")" && pwd -W)/$(basename "$test_shell")
fi
cat > "$temporary/test.mk" <<'MAKE'
include Makefile
parallel-check:
	$(call parallel_for,one two three four,printf "start %s\n" "$$item" >> "$$PARALLEL_TEST_LOG"; sleep 0.3; printf "end %s\n" "$$item" >> "$$PARALLEL_TEST_LOG",Pool check,2)
parallel-failure:
	$(call parallel_for,one broken three,if [ "$$item" = broken ]; then echo expected-failure; exit 7; fi,Pool check,2)
parallel-invalid:
	$(call parallel_for,one,true,Pool check,0)
MAKE
run_make() {
    make -s -f "$temporary/test.mk" "SHELL=$test_shell" "$@"
}
run_make parallel-check
awk '
    $1 == "start" { active++; started++; if (active > maximum) maximum = active }
    $1 == "end" { active-- }
    END { if (maximum != 2 || active != 0 || started != 4) exit 1 }
' "$PARALLEL_TEST_LOG"
if run_make parallel-failure > "$temporary/failure" 2>&1; then
    echo 'A failed worker was reported as successful' >&2
    exit 1
fi
case "$(cat "$temporary/failure")" in
    *"[FAIL] broken"*"expected-failure"*) ;;
    *) cat "$temporary/failure" >&2; exit 1 ;;
esac
if run_make parallel-invalid > "$temporary/invalid" 2>&1; then
    echo 'A zero worker limit was accepted' >&2
    exit 1
fi
case "$(cat "$temporary/invalid")" in
    *"Parallel job limit must be a positive integer"*) ;;
    *) cat "$temporary/invalid" >&2; exit 1 ;;
esac
echo 'Parallel worker limit, completion and failure reporting: passed'
