"""Compare headless console backends with the byte relay.

This compares transport+parser wall time, including child startup and shutdown.
It does not measure GUI rendering. Run an optimized binary and keep history fixed.
"""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", "-b", type=Path, required=True)
    parser.add_argument("--relay", "-r", type=Path, required=True,
                        help="shitty/dev/compare_windows.py")
    parser.add_argument("--payload", "-p", type=Path, required=True)
    parser.add_argument("--cat", "-c", type=Path, required=True)
    parser.add_argument("--runs", "-n", type=int, default=3)
    parser.add_argument("--history", "-s", type=int, default=500)
    parser.add_argument("--pipeline", "-P", action="store_true", help="include the embedded parser-thread path")
    parser.add_argument("--report", "-o", type=Path, required=True)
    result = parser.parse_args()
    if result.runs < 1 or result.history < 0:
        parser.error("runs must be positive and history nonnegative")
    return result


def measure(args, work, mode, iteration):
    result = work / f"relay-{mode}-{iteration}.json"
    child_args = [sys.executable, str(args.relay.resolve()), "--child", "cat",
                  str(result), str(args.payload.resolve()), str(args.cat.resolve())]
    if mode == "pipe":
        started = time.perf_counter()
        with subprocess.Popen(child_args, stdout=subprocess.PIPE) as child:
            total = 0
            while chunk := child.stdout.read(131072):
                total += len(chunk)
            code = child.wait()
        if code:
            raise RuntimeError(f"pipe child exited with {code}")
        data = {"backend": "pipe", "source_bytes": total,
                "elapsed_seconds": time.perf_counter() - started}
    else:
        command = [str(args.binary.resolve()), "--command",
                   subprocess.list2cmdline(child_args), "--history", str(args.history),
                   "--timeout", "30"]
        if mode == "drain":
            command.append("--drain")
        elif mode == "pipeline":
            command.append("--pipeline")
        completed = subprocess.run(command, capture_output=True, text=True,
                                   check=True, timeout=40)
        data = json.loads(completed.stdout)
    expected = args.payload.stat().st_size
    if data["source_bytes"] != expected or json.loads(result.read_text())["bytes"] != expected:
        raise RuntimeError("source-byte count mismatch")
    data["mode"] = mode
    data["iteration"] = iteration
    return data


def main():
    args = arguments()
    with args.payload.open("rb") as payload:
        digest = hashlib.file_digest(payload, "sha256").hexdigest()
    measurements = []
    with tempfile.TemporaryDirectory(prefix="zetta-condrv-") as directory:
        work = Path(directory)
        for iteration in range(args.runs):
            # Rotate ordering to reduce a systematic warm-cache/order advantage.
            modes = ["pipe", "drain", "parser"]
            if args.pipeline:
                modes.append("pipeline")
            modes = modes[iteration % len(modes):] + modes[:iteration % len(modes)]
            for mode in modes:
                data = measure(args, work, mode, iteration)
                measurements.append(data)
                print(f"{mode}: {data['elapsed_seconds']:.3f}s", flush=True)
    report = {"version": 1, "headless": True, "history": args.history,
              "payload_bytes": args.payload.stat().st_size, "payload_sha256": digest,
              "binary": str(args.binary.resolve()), "relay": str(args.relay.resolve()),
              "platform": platform.platform(), "machine": platform.machine(),
              "python": sys.version, "measurements": measurements}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
