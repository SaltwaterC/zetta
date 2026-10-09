#!/usr/bin/env python3
"""Measure generated payloads and shell phases using a fresh optimized binary.

Each shell runs a fixed init completion probe. Phase times exclude process launch
and generation of the initial startup payload. First completion includes lazy
payload generation/evaluation and, for Zsh, compinit. No cache purging is used.
"""
import argparse
import datetime
import json
import hashlib
import os
import platform
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import threading


def generate(binary, shell, completions=False):
    start = time.perf_counter()
    payload = subprocess.check_output(
        [str(binary), "init", shell] + (["--completions"] if completions else []),
        timeout=30,
    )
    return payload, (time.perf_counter() - start) * 1000


def shell_driver(shell, eager):
    if shell == "bash":
        return ["--noprofile", "--norc"], [
            'source "$STARTUP"',
            'COMP_WORDS=(zetta init zs); COMP_CWORD=2; ' + ('_zetta_complete; ' if eager else '__zetta_lazy_complete; ') +
            '[[ ${COMPREPLY[*]} == zsh ]] || exit 11',
            '_zetta_complete; [[ ${COMPREPLY[*]} == zsh ]] || exit 12',
        ]
    if shell == "fish":
        probe = "string match -q -- 'zsh*' (complete -C 'zetta init zs'); or exit 11"
        return ["--no-config"], ['source "$STARTUP"', probe, probe]
    if shell == "zsh":
        return ["-df"], [
            'source "$STARTUP"',
            ('' if eager else 'autoload -Uz compinit; compinit; __zetta_register_lazy_completions; ') +
            'function compadd { print -r -- "$@" > "$RESULT"; }; '
            'words=(zetta init zs); CURRENT=3; ' + ('_zetta; ' if eager else '__zetta_lazy_complete; ') +
            '[[ $(<"$RESULT") == *zsh* ]] || exit 11',
            '_zetta; [[ $(<"$RESULT") == *zsh* ]] || exit 12',
        ]
    probe = "$r = [System.Management.Automation.CommandCompletion]::CompleteInput('zetta init zs', 13, $null); if ($r.CompletionMatches.CompletionText -notcontains 'zsh') { exit 11 }"
    return ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"], [
        "Get-Content -Raw -LiteralPath $env:STARTUP | Invoke-Expression", probe, probe,
    ]


def measure_shell(executable, shell, environment, eager):
    flags, phases = shell_driver(shell, eager)
    process = subprocess.Popen(
        [executable, *flags], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, env=environment,
    )
    watchdog = threading.Timer(30, process.kill)
    watchdog.start()
    phase_names = ("evaluation_ms", "first_completion_ms", "subsequent_completion_ms")

    def marker_command(marker):
        if shell == "powershell":
            return f"[Console]::WriteLine('{marker}')"
        return f"printf '\\n{marker}\\n'"

    commands = [marker_command("ZETTA_READY")]
    for name, command in zip(phase_names, phases):
        commands.extend((command, marker_command(f"ZETTA_BENCH_{name}")))
    commands.append("exit")
    results = {}
    try:
        # Fish parses a noninteractive stdin script through EOF before executing it.
        process.stdin.write("\n".join(commands) + "\n")
        process.stdin.close()
        process.stdin = None
        start = None
        pending = iter(phase_names)
        name = next(pending)
        for line in process.stdout:
            now = time.perf_counter()
            if "ZETTA_READY" in line:
                start = now
            elif f"ZETTA_BENCH_{name}" in line:
                if start is None:
                    raise RuntimeError("Missing startup marker")
                results[name] = (now - start) * 1000
                start = now
                name = next(pending, "done")
        _, stderr = process.communicate(timeout=30)
        if process.returncode or len(results) != 3:
            raise RuntimeError(f"{shell} exited {process.returncode}: {stderr}")
    finally:
        watchdog.cancel()
        if process.poll() is None:
            process.kill()
            process.wait()
    results["evaluation_plus_first_completion_ms"] = results["evaluation_ms"] + results["first_completion_ms"]
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/zetta"))
    parser.add_argument("--output", type=Path, default=Path("artifacts/shell-integration-performance.json"))
    parser.add_argument("--eager", action="store_true", help="Measure a pre-lazy-loading binary for comparison")
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    binary = args.binary.resolve(strict=True)
    report = {"schema_version": 1, "binary": str(binary),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "generated_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "platform": platform.platform(), "runs": args.runs, "eager": args.eager,
              "probe": "programmatic zetta init zs; Zsh completion display stubbed",
              "samples": [], "skipped": []}
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        # Completion generation resolves zetta through PATH just as startup does.
        name = "zetta.exe" if os.name == "nt" else "zetta"
        shutil.copy2(binary, root / name)
        environment = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                           STARTUP=str(root / "startup"), RESULT=str(root / "result"))
        for shell, candidates in {"bash": ["bash"], "fish": ["fish"], "zsh": ["zsh"],
                                  "powershell": ["powershell.exe", "pwsh.exe", "pwsh"]}.items():
            available = [shutil.which(candidate) for candidate in candidates if shutil.which(candidate)]
            if not available:
                report["skipped"].append(shell)
            for run in range(args.runs):
                startup, generation = generate(binary, shell)
                completion, completion_generation = (b"", 0) if args.eager else generate(binary, shell, True)
                (root / "startup").write_bytes(startup)
                sample = {"shell": shell, "run": run, "startup_bytes": len(startup),
                          "completion_bytes": len(completion), "generation_ms": generation,
                          "completion_generation_ms": completion_generation}
                report["samples"].append(sample)
                for executable in available:
                    runtime = measure_shell(executable, shell, environment, args.eager)
                    runtime["initialization_plus_first_completion_ms"] = generation + runtime["evaluation_plus_first_completion_ms"]
                    sample.setdefault("runtimes", []).append({"executable": executable, **runtime})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Wrote {args.output}")


if __name__ == "__main__":
    main()
