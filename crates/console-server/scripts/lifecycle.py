"""Child fixture for console lifecycle checks; argv[1] picks the check."""
import ctypes, os, subprocess, sys, time
from ctypes import wintypes as W

k = ctypes.WinDLL("kernel32", use_last_error=True)
HANDLER = ctypes.WINFUNCTYPE(W.BOOL, W.DWORD)
seen = []


@HANDLER
def handler(event):
    seen.append(event)
    if event == 2 and len(sys.argv) > 2:  # CTRL_CLOSE_EVENT
        with open(sys.argv[2], "w") as marker:
            marker.write("closed")
    return True


k.SetConsoleCtrlHandler(None, False)
k.SetConsoleCtrlHandler(handler, True)
check = sys.argv[1]
if check == "close":
    print("READY", flush=True)
    time.sleep(30)
elif check == "generate":
    # Ctrl+Break to every process in the console (group 0).
    ok = k.GenerateConsoleCtrlEvent(1, 0)
    deadline = time.monotonic() + 5
    while 1 not in seen and time.monotonic() < deadline:
        time.sleep(0.01)
    print("GENERATED", ok, seen, flush=True)
elif check == "detach":
    flags = 0x00000008 | 0x00000200  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], creationflags=flags)
    with open(sys.argv[2], "w") as marker:
        marker.write(str(child.pid))
    print("READY", flush=True)
    time.sleep(30)
elif check == "window":
    hwnd = k.GetConsoleWindow()
    user = ctypes.WinDLL("user32")
    print("WINDOW", bool(hwnd), user.IsWindow(hwnd), user.IsWindowVisible(hwnd), flush=True)
