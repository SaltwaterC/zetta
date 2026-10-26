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
	@echo "[FOLLOW-UP] Pool check recipe continued"
parallel-external:
	$(call parallel_for,one two three,powershell.exe -NoProfile -Command "Start-Sleep -Milliseconds 300",External check,2)
	@echo "[FOLLOW-UP] External worker recipe continued"
parallel-failure:
	$(call parallel_for,one broken three,if [ "$$item" = broken ]; then echo expected-failure; exit 7; fi,Pool check,2)
parallel-invalid:
	$(call parallel_for,one,true,Pool check,0)
MAKE
run_make() {
    make -s -f "$temporary/test.mk" "SHELL=$test_shell" "$@"
}
run_make_errexit() {
    make -s -f "$temporary/test.mk" "SHELL=$test_shell" '.SHELLFLAGS=-ec' "$@"
}
run_make parallel-check >"$temporary/completion"
case "$(cat "$temporary/completion")" in
	*"[WAIT] Pool check: reaping completed workers"*"[WAIT] Pool check: collecting worker results"*"[DONE] Pool check: workers exited, results collected, temporary files removed"*"[FOLLOW-UP] Pool check recipe continued"*) ;;
    *) cat "$temporary/completion" >&2; exit 1 ;;
esac
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
	*"[FAIL] broken"*"expected-failure"*"[DONE] Pool check: workers exited, results collected, temporary files removed"*) ;;
    *) cat "$temporary/failure" >&2; exit 1 ;;
esac
if grep -Fq 'worker exited before reporting completion' "$temporary/failure"; then
    echo 'A failed worker bypassed normal status reporting' >&2
    exit 1
fi
if run_make_errexit parallel-failure >"$temporary/failure-errexit" 2>&1; then
    echo 'A failed worker under shell errexit was reported as successful' >&2
    exit 1
fi
case "$(cat "$temporary/failure-errexit")" in
	*"[FAIL] broken"*"expected-failure"*"[DONE] Pool check: workers exited, results collected, temporary files removed"*) ;;
	*) cat "$temporary/failure-errexit" >&2; exit 1 ;;
esac
if grep -Fq 'worker exited before reporting completion' "$temporary/failure-errexit"; then
    echo 'A failed worker under shell errexit bypassed normal status reporting' >&2
    exit 1
fi
run_make parallel-external >"$temporary/external"
case "$(cat "$temporary/external")" in
	*"[DONE] External check: workers exited, results collected, temporary files removed"*"[FOLLOW-UP] External worker recipe continued"*) ;;
	*) cat "$temporary/external" >&2; exit 1 ;;
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
