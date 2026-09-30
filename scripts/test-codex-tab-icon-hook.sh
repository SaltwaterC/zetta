#!/bin/sh
# Exercise the noninteractive hook used by native POSIX shells and WSL.
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
hook=${1:-"$script_directory/codex-tab-icon-hook.sh"}
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT HUP INT TERM

cat > "$temporary/zetta" <<'FAKE_ZETTA'
#!/bin/sh
printf '%s\n' "$*" > "$ZETTA_HOOK_TEST_OUTPUT"
# Model a host that can accept the request immediately but cannot finish
# applying it within the hook's deadline.
[ "$1" = tabicon ] && [ "$2" = --queue ]
FAKE_ZETTA
chmod +x "$temporary/zetta"

export ZETTA_HOOK_TEST_OUTPUT="$temporary/output"
export ZETTA_HOST_EXECUTABLE="$temporary/zetta"

assert_icon() {
    state=$1
    expected=$2
    rm -f -- "$ZETTA_HOOK_TEST_OUTPUT"
    printf '%s\n' '{}' | sh "$hook" "$state" 2> "$temporary/errors"
    if [ ! -f "$ZETTA_HOOK_TEST_OUTPUT" ] ||
        [ "$(cat "$ZETTA_HOOK_TEST_OUTPUT")" != "$expected" ] ||
        [ -s "$temporary/errors" ]; then
        printf 'Failed hook state: %s\n' "$state" >&2
        cat "$temporary/errors" >&2
        exit 1
    fi
}

assert_icon idle 'tabicon --queue ai_open_ai'
assert_icon prompt 'tabicon --queue ai_open_ai_compat'
assert_icon reset 'tabicon --queue --reset'

# Noninteractive shells also need the PATH fallback when there is no owner.
unset ZETTA_HOST_EXECUTABLE
export PATH="$temporary:$PATH"
assert_icon idle 'tabicon --queue ai_open_ai'
assert_icon reset 'tabicon --queue --reset'

# The WSL route must hand the original payload to the Windows hook before
# parsing it, and use the supplied script path even when it contains spaces.
cat > "$temporary/wslpath" <<'WSLPATH'
#!/bin/sh
printf '%s\n' "$2"
WSLPATH
cat > "$temporary/powershell.exe" <<'POWERSHELL'
#!/bin/sh
printf '%s\n' "$*" > "$ZETTA_HOOK_TEST_OUTPUT"
cat > "$ZETTA_HOOK_TEST_OUTPUT.payload"
printf '%s\n' "$WSLENV" > "$ZETTA_HOOK_TEST_OUTPUT.wslenv"
printf '%s\n' "$@" > "$ZETTA_HOOK_TEST_OUTPUT.arguments"
POWERSHELL
chmod +x "$temporary/wslpath" "$temporary/powershell.exe"
export ZETTA_HOST_EXECUTABLE="$temporary/host.exe"
export WSL_DISTRO_NAME='Ubuntu Development'
export WSLENV='USER/u:ZETTA_PROCESS_ID/u:ZETTA_ATTENTION_ID/up:ZETTA_PROCESS_ID/l'
mkdir "$temporary/hook scripts"
cp "$hook" "$temporary/hook scripts/codex-tab-icon-hook.sh"
touch "$temporary/hook scripts/codex-tab-icon-hook.ps1"
payload='{"permission_mode":"default","transcript_path":"/tmp/planning transcript.jsonl","turn_id":"current-turn"}'
for state in idle prompt reset; do
    rm -f -- "$ZETTA_HOOK_TEST_OUTPUT" "$ZETTA_HOOK_TEST_OUTPUT.payload"
    printf '%s\n' "$payload" |
        sh "$temporary/hook scripts/codex-tab-icon-hook.sh" "$state"
    expected="-NoProfile -ExecutionPolicy Bypass -File $temporary/hook scripts/codex-tab-icon-hook.ps1 $state -WslDistribution $WSL_DISTRO_NAME"
    [ "$(cat "$ZETTA_HOOK_TEST_OUTPUT")" = "$expected" ]
    [ "$(cat "$ZETTA_HOOK_TEST_OUTPUT.payload")" = "$payload" ]
    [ "$(cat "$ZETTA_HOOK_TEST_OUTPUT.wslenv")" = 'USER/u:ZETTA_PROCESS_ID:ZETTA_ATTENTION_ID' ]
    [ "$(tail -n 1 "$ZETTA_HOOK_TEST_OUTPUT.arguments")" = "$WSL_DISTRO_NAME" ]
done

printf '%s\n' 'Codex shell tab-icon hook checks passed.'
