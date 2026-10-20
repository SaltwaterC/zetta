"""Adapt the imported Python harness to Zetta's headless terminal core.

Only observations provided by the engine are exposed. Missing harness features
raise SkipTest; values must never be invented to make a foreign assertion pass.
"""

import json
from dataclasses import make_dataclass
from functools import lru_cache
import queue
import threading
import subprocess
import unittest
from types import SimpleNamespace

BINARY = None
REPLY_TIMEOUT = 10.0


class Observation(SimpleNamespace):
    def __eq__(self, other):
        return type(self) is type(other) and vars(self) == vars(other)

    def __getattr__(self, name):
        raise unittest.SkipTest(f"Zetta core adapter cannot observe {name}")


class Snapshot(Observation):
    def cell(self, column, row):
        if not (0 <= column < self.columns and 0 <= row < self.rows):
            raise IndexError("cell coordinate is outside the page")
        return self.cells[row * self.columns + column]


@lru_cache(maxsize=8)
def observation_type(base, names):
    # dataclasses.asdict/fields see exactly the observations the engine supplies.
    # Missing frontend metadata remains unavailable instead of acquiring defaults.
    return make_dataclass(base.__name__, [(name, object) for name in names],
                          bases=(base,), init=False, eq=False, repr=False)


class Zetta:
    def __init__(self, columns=80, rows=24, save_lines=500, **options):
        # UI options belong to Shitty's frontend and cannot configure this core.
        if options:
            raise unittest.SkipTest("Shitty frontend options: " + ", ".join(options))
        self.process = subprocess.Popen(
            [str(BINARY)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            text=True, encoding="utf-8",
        )
        self.replies = queue.Queue()
        self.reader = threading.Thread(target=self._receive, daemon=True)
        self.reader.start()
        try:
            self.request("new", columns=columns, rows=rows, history=save_lines)
        except BaseException:
            self.close()
            raise

    def _receive(self):
        try:
            for line in self.process.stdout:
                self.replies.put(line)
        finally:
            self.replies.put(None)

    def request(self, op, **fields):
        self.process.stdin.write(json.dumps(dict(op=op, **fields)) + "\n")
        self.process.stdin.flush()
        try:
            line = self.replies.get(timeout=REPLY_TIMEOUT)
        except queue.Empty:
            self.process.kill()
            self.process.wait()
            raise RuntimeError(f"Zetta adapter timed out handling {op}") from None
        if not line:
            raise RuntimeError(f"Zetta adapter exited: {self.process.wait()}")
        reply = json.loads(line)
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply["result"]

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        finally:
            self.reader.join(timeout=5)
            self.process.stdout.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def write(self, output):
        self.request("write", data=list(output))

    def write_chunks(self, *chunks):
        for chunk in chunks:
            self.write(chunk)

    def feed_chunks(self, *chunks):
        if not chunks or any(not chunk for chunk in chunks):
            raise ValueError("PTY chunks must be non-empty")
        self.write_chunks(*chunks)

    def snapshot(self):
        result = self.request("snapshot")
        # Alacritty retains tabs for copy/selection; visually each marker is a
        # blank cell. The imported snapshots expose the visual character.
        result["lines"] = [line.replace("\t", " ") for line in result["lines"]]
        symbols = result.pop("symbols", None)
        indices = result["cells"]
        if symbols is not None:
            result["cells"] = symbols
        for cell in result["cells"]:
            if cell["char"] == "\t":
                cell["char"] = " "
            for name in ("foreground", "background", "underline_color", "grapheme"):
                cell[name] = tuple(cell[name])
        cell_type = observation_type(Observation, tuple(result["cells"][0]))
        if symbols is None:
            result["cells"] = [cell_type(**cell) for cell in result["cells"]]
        else:
            result["cells"] = [cell_type(**symbols[index]) for index in indices]
        return observation_type(Snapshot, tuple(result))(**result)

    model_snapshot = snapshot

    def read_input(self):
        reply = self.request("read_input")
        if reply["unavailable"]:
            raise unittest.SkipTest("Zetta core adapter cannot supply " + ", ".join(reply["unavailable"]))
        return bytes(reply["bytes"])

    read_all_input = read_input

    def resize(self, columns, rows):
        self.request("resize", columns=columns, rows=rows)

    def hard_reset(self):
        self.write(b"\x1bc")

    def screen_text(self):
        return "".join("".join(c if " " <= c <= "~" else " " for c in line) + "\n"
                       for line in self.snapshot().lines)

    def hyperlink(self, column, row):
        return self.request("hyperlink", column=column, row=row) or ""

    def hyperlink_bytes(self, column, row):
        return self.hyperlink(column, row).encode("utf-8")

    def page_up(self):
        self.wheel_up(self.snapshot().rows // 2)

    def page_down(self):
        self.wheel_down(self.snapshot().rows // 2)

    def wheel_up(self, count=1):
        self._scroll(count)

    def wheel_down(self, count=1):
        self._scroll(-count)


    def _scroll(self, count):
        reply = self.request("scroll", delta=count)
        if reply and reply.get("unavailable"):
            raise unittest.SkipTest(reply["unavailable"])


def unsupported(name):
    def skip(*args, **kwargs):
        raise unittest.SkipTest(f"Zetta core adapter does not implement {name}")
    return skip


def install(harness, binary):
    global BINARY
    BINARY = binary
    # Restrict skips to actual upstream methods so spelling mistakes remain errors.
    for name in vars(harness.Shitty):
        if not name.startswith("_") and not hasattr(Zetta, name):
            setattr(Zetta, name, unsupported(name))
    harness.Shitty = Zetta
    harness.run_startup_failure = unsupported("Shitty executable startup options")
