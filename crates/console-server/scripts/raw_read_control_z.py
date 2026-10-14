"""Child fixture: ReadFile on the console ends at a leading Ctrl+Z in any mode."""
import ctypes
from ctypes import wintypes

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.GetStdHandle.argtypes = [wintypes.DWORD]
kernel.GetStdHandle.restype = wintypes.HANDLE
kernel.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel.ReadFile.argtypes = [
    wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p
]

handle = kernel.GetStdHandle(-10 & 0xFFFFFFFF)
# Raw and unprocessed: ReadConsole would return the ^Z, but ReadFile always
# treats one at the start of the read as end of file.
assert kernel.SetConsoleMode(handle, 0)
print("CONTROL_Z_READY", flush=True)
buffer = ctypes.create_string_buffer(8)
read = wintypes.DWORD(99)
assert kernel.ReadFile(handle, buffer, 8, ctypes.byref(read), None)
print(f"CONTROL_Z_READ {read.value}", flush=True)
