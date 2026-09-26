"""Loopback-only remote program; no shell startup files or user configuration."""
import json
import os
import signal
import subprocess
import sys
import tty


def diagnostics():
    expected = {
        "LANG": sys.argv[3], "LANGUAGE": "", "LC_CTYPE": sys.argv[3],
        "LC_NUMERIC": "", "LC_ZOSH_TEST": "remote-extension",
    }
    actual = {k: v for k, v in os.environ.items()
              if k in ("LANG", "LANGUAGE") or k.startswith("LC_")}
    assert actual == expected, (actual, expected)
    assert os.environ["TERM"] == "xterm-256color"
    assert os.environ["COLORTERM"] == "truecolor"
    commands = [["locale"]]
    if sys.platform.startswith("linux"):
        commands.append(["perl", "-e", "print qq(perl-ready\\n)"])
    results = []
    for command in commands:
        result = subprocess.run(command, capture_output=True, timeout=5)
        results.append([command, result.returncode, result.stdout.hex(), result.stderr.hex()])
    return json.dumps([actual, results], sort_keys=True)


report = diagnostics()
if sys.argv[1] == "direct":
    print(report)
    sys.exit(0)
with open(sys.argv[2], "w") as output:
    output.write(report + "\n")
signal.alarm(15)
tty.setraw(0)
colours = []
for index, (kind, terminator) in enumerate([(10, b"\x07"), (11, b"\x1b\\"),
                                           (10, b"\x1b\\"), (11, b"\x07")]):
    os.write(1, b"\x1b]" + str(kind).encode() + b";?" + terminator)
    response = b""
    while not response.endswith((b"\x07", b"\x1b\\")):
        response += os.read(0, 1)
        assert len(response) < 128
    prefix = b"\x1b]" + str(kind).encode() + b";rgb:"
    assert response.startswith(prefix), response
    body = response[len(prefix):].rstrip(b"\x07\x1b\\")
    channels = body.split(b"/")
    assert len(channels) == 3 and all(len(c) == 4 for c in channels), response
    colours.append(";".join(str(int(c, 16) >> 8) for c in channels))
    if index % 2:
        row = index // 2 + 1
        os.write(1, (f"\x1b[{row};1H\x1b[38;2;{colours[-2]}m"
                     f"\x1b[48;2;{colours[-1]}mX\x1b[0m").encode())
os.write(1, b"\x1b[4;1HCOLOUR-LOCALE-DONE")
os.read(0, 1)  # Keep the final framebuffer alive until the client has checked it.
