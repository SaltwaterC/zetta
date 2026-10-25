#!/usr/bin/env sh

# Exercise artifact checks and build plans without installing or compiling.
set -eu

repository=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/zetta-build-wiring.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

fail() {
    printf 'build wiring test failed: %s\n' "$1" >&2
    exit 1
}

# Pin capability flags so the caller's environment cannot change expectations.
run_make() {
    make --no-print-directory -C "$repository" OS=Unix UNAME_S="$platform" CARGO_BUILD_JOBS=2 \
        RELEASE="$release" SERIAL=1 HTTP=1 TFTP=1 TFTP_SERVER=1 TFTP_CLIENT=1 \
        NOTIFY=1 CLIPBOARD=1 SYNTAX_HIGHLIGHTING=1 ZMUX=1 \
        SESSION_PERSISTENCE=1 WORKTREE=1 ZDIFF=1 ZOSH=1 ZOSH_CLIENT=1 ZOSH_SERVER=1 \
        "$@"
}

for platform in Linux Darwin; do
    for release in 0 1; do
        artifact_dir="$test_root/$platform/$release"
        mkdir -p "$artifact_dir"
        for binary in zetta zntfy ztftp zhttp zcopy zpaste zmux zosh zosh-server zwt zpager; do
            : > "$artifact_dir/$binary"
            chmod +x "$artifact_dir/$binary"
        done
        run_make check-build-binaries BUILD_TARGET_DIR="$artifact_dir"

        # An existing application must not hide a missing companion tool.
        rm "$artifact_dir/ztftp" "$artifact_dir/zhttp"
        if run_make check-build-binaries BUILD_TARGET_DIR="$artifact_dir" > "$test_root/output" 2>&1; then
            fail "$platform accepted an incomplete build"
        fi
        for binary in ztftp zhttp; do
            grep -F "Missing executable: $artifact_dir/$binary" "$test_root/output" > /dev/null ||
                fail "$platform did not report missing $binary"
        done
        profile_arg=''
        if [ "$release" = 1 ]; then profile_arg=' RELEASE=1'; fi
        grep -F "make build$profile_arg'" "$test_root/output" > /dev/null ||
            fail "$platform suggested the wrong build profile"

        # Tools disabled for installation are not required artifacts.
        run_make check-build-binaries BUILD_TARGET_DIR="$artifact_dir" HTTP=0 TFTP_SERVER=0 TFTP_CLIENT=0

        for binary in zntfy zcopy zpaste zmux zosh zosh-server zwt zpager; do
            rm "$artifact_dir/$binary"
        done
        run_make check-build-binaries BUILD_TARGET_DIR="$artifact_dir" \
            HTTP=0 TFTP_SERVER=0 TFTP_CLIENT=0 NOTIFY=0 CLIPBOARD=0 \
            ZMUX=0 WORKTREE=0 ZDIFF=0 ZOSH_CLIENT=0 ZOSH_SERVER=0

        run_make -n build > "$test_root/plan"
        for crate in zntfy ztftp zhttp zclip; do
            grep -F -- "--manifest-path crates/$crate/Cargo.toml --target-dir target" "$test_root/plan" > /dev/null ||
                fail "$platform build omitted $crate from the shared artifact directory"
        done
        run_make -n build-tools TOOLS=zpager > "$test_root/tools-plan"
        grep -F -- "--manifest-path crates/zpager/Cargo.toml --target-dir target/tools --bin zpager" "$test_root/tools-plan" > /dev/null ||
            fail "$platform standalone tools build omitted zpager"
        if [ "$release" = 1 ]; then
            [ "$(grep -c 'build --release' "$test_root/plan")" -eq 5 ] ||
                fail "$platform did not apply release to every Cargo build"
        fi
    done
done

printf 'Unix build wiring tests passed.\n'
