"""Child fixture for console-server compatibility checks; uses only its own console."""
import ctypes
from ctypes import wintypes

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
HANDLE = wintypes.HANDLE
DWORD = wintypes.DWORD
kernel.GetStdHandle.argtypes = [DWORD]
kernel.GetStdHandle.restype = HANDLE
kernel.GetConsoleMode.argtypes = [HANDLE, ctypes.POINTER(DWORD)]
kernel.GetNumberOfConsoleInputEvents.argtypes = [HANDLE, ctypes.POINTER(DWORD)]
kernel.WriteFile.argtypes = [HANDLE, ctypes.c_void_p, DWORD, ctypes.POINTER(DWORD), ctypes.c_void_p]
kernel.WriteConsoleW.argtypes = [HANDLE, ctypes.c_void_p, DWORD, ctypes.POINTER(DWORD), ctypes.c_void_p]
kernel.WriteConsoleA.argtypes = kernel.WriteConsoleW.argtypes
kernel.CreateFileW.argtypes = [
    wintypes.LPCWSTR, DWORD, DWORD, ctypes.c_void_p, DWORD, DWORD, HANDLE
]
kernel.CreateFileW.restype = HANDLE
kernel.CloseHandle.argtypes = [HANDLE]
kernel.SetConsoleOutputCP.argtypes = [DWORD]
kernel.GetConsoleOutputCP.restype = DWORD
kernel.GetConsoleCP.restype = DWORD
kernel.ReadConsoleW.argtypes = [HANDLE, ctypes.c_void_p, DWORD, ctypes.POINTER(DWORD), ctypes.c_void_p]


def checked(value):
    if not value:
        raise ctypes.WinError(ctypes.get_last_error())
    return value


output = checked(kernel.GetStdHandle(-11 & 0xFFFFFFFF))
input_handle = checked(kernel.GetStdHandle(-10 & 0xFFFFFFFF))
mode = DWORD()
checked(kernel.GetConsoleMode(output, ctypes.byref(mode)))
assert mode.value & 7 == 7
ready = DWORD()
checked(kernel.GetNumberOfConsoleInputEvents(input_handle, ctypes.byref(ready)))
assert ready.value == 0
assert not kernel.GetNumberOfConsoleInputEvents(output, ctypes.byref(ready))
# A new console starts in the OEM code page, as ConPTY does; any installed
# code page can be selected, and the rest of this probe works in UTF-8.
kernel.GetOEMCP.restype = DWORD
kernel.SetConsoleCP.argtypes = [DWORD]
assert kernel.GetConsoleOutputCP() == kernel.GetOEMCP()
assert kernel.GetConsoleCP() == kernel.GetOEMCP()
checked(kernel.SetConsoleOutputCP(437))
assert kernel.GetConsoleOutputCP() == 437
assert not kernel.SetConsoleOutputCP(1), "uninstalled code pages must fail"
checked(kernel.SetConsoleOutputCP(65001))
checked(kernel.SetConsoleCP(65001))
for name in ("CONOUT$", "CONIN$"):
    handle = kernel.CreateFileW(name, 0xC0000000, 3, None, 3, 0, None)
    assert handle not in (None, ctypes.c_void_p(-1).value)
    checked(kernel.GetConsoleMode(handle, ctypes.byref(mode)))
    checked(kernel.CloseHandle(handle))
wide = "wide: \u03bb \U0001f642\r\n".encode("utf-16-le")
wide_buffer = ctypes.create_string_buffer(wide)
written = DWORD()
checked(kernel.WriteConsoleW(output, wide_buffer, len(wide) // 2, ctypes.byref(written), None))
assert written.value == len(wide) // 2
for function, data in [
    (kernel.WriteConsoleA, b"ansi: \xce\xbb\r\n"),
    (kernel.WriteFile, b"\x1b[31mraw\x1b[0m\r\n"),
]:
    buffer = ctypes.create_string_buffer(data)
    checked(function(output, buffer, len(data), ctypes.byref(written), None))
    assert written.value == len(data)
buffer = ctypes.create_unicode_buffer(8)
assert not kernel.ReadConsoleW(input_handle, buffer, 8, ctypes.byref(written), None)
# Exit success only after every assertion and every write have completed.

class Coord(ctypes.Structure):
    _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short)]


class Rect(ctypes.Structure):
    _fields_ = [("left", ctypes.c_short), ("top", ctypes.c_short),
                ("right", ctypes.c_short), ("bottom", ctypes.c_short)]


class BufferInfo(ctypes.Structure):
    _fields_ = [("size", Coord), ("cursor", Coord), ("attributes", wintypes.WORD),
                ("window", Rect), ("maximum", Coord)]


kernel.GetConsoleScreenBufferInfo.argtypes = [HANDLE, ctypes.POINTER(BufferInfo)]
info = BufferInfo()
checked(kernel.GetConsoleScreenBufferInfo(output, ctypes.byref(info)))
assert (info.size.x, info.size.y) == (80, 24)
assert (info.window.right, info.window.bottom) == (79, 23)
assert (info.cursor.x, info.cursor.y) == (0, 3)
kernel.GetConsoleProcessList.argtypes = [ctypes.POINTER(DWORD), DWORD]
pids = (DWORD * 8)()
assert kernel.GetConsoleProcessList(pids, 8) == 1
kernel.GetCurrentProcessId.restype = DWORD
assert pids[0] == kernel.GetCurrentProcessId()

# Titles are API calls, not output bytes. Exercise both encodings and truncation.
kernel.SetConsoleTitleW.argtypes = [wintypes.LPCWSTR]
kernel.SetConsoleTitleA.argtypes = [ctypes.c_char_p]
kernel.GetConsoleTitleW.argtypes = [wintypes.LPWSTR, DWORD]
kernel.GetConsoleTitleW.restype = DWORD
kernel.GetConsoleTitleA.argtypes = [ctypes.c_void_p, DWORD]
kernel.GetConsoleTitleA.restype = DWORD
checked(kernel.SetConsoleTitleW("wide title: λ 🙂"))
title = ctypes.create_unicode_buffer(128)
assert kernel.GetConsoleTitleW(title, 128) == len("wide title: λ 🙂".encode("utf-16-le")) // 2
assert title.value == "wide title: λ 🙂"
checked(kernel.SetConsoleTitleA("ansi title: λ".encode("utf-8")))
ansi = ctypes.create_string_buffer(128)
assert kernel.GetConsoleTitleA(ansi, 128) == len("ansi title: λ".encode("utf-8"))
assert ansi.value.decode("utf-8") == "ansi title: λ"
small = ctypes.create_string_buffer(5)
assert kernel.GetConsoleTitleA(small, 5) == len("ansi title: λ".encode("utf-8"))
assert small.value == b"ansi"
checked(kernel.SetConsoleTitleW(""))
assert kernel.GetConsoleTitleW(title, 128) == 0
assert title.value == ""

# The authoritative title also follows an OSC title written through the parser.
osc = b"\x1b]2;VT title\x07"
checked(kernel.WriteFile(output, ctypes.create_string_buffer(osc), len(osc), ctypes.byref(written), None))
assert kernel.GetConsoleTitleW(title, 128) == len("VT title")
assert title.value == "VT title"
