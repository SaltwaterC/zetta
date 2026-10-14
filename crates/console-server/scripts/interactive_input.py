"""Native input-record fixture for the embedded Session API."""
import ctypes
from ctypes import wintypes
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.GetStdHandle.argtypes = [wintypes.DWORD]
kernel.GetStdHandle.restype = wintypes.HANDLE
kernel.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel.ReadConsoleInputW.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
handle = kernel.GetStdHandle(-10 & 0xffffffff)
assert kernel.SetConsoleMode(handle, 8)
print("RECORDS_READY", flush=True)
records = []
for _ in range(3):
    buffer = ctypes.create_string_buffer(20)
    count = wintypes.DWORD()
    assert kernel.ReadConsoleInputW(handle, buffer, 1, ctypes.byref(count))
    assert count.value == 1
    records.append(buffer.raw)
assert int.from_bytes(records[0][:2], "little") == 4
assert int.from_bytes(records[0][4:6], "little") == 90
assert int.from_bytes(records[0][6:8], "little") == 35
for index, down in [(1, 1), (2, 0)]:
    record = records[index]
    assert int.from_bytes(record[:2], "little") == 1
    assert int.from_bytes(record[4:8], "little") == down
    assert int.from_bytes(record[8:10], "little") == 3
    assert int.from_bytes(record[10:12], "little") == 65
    assert int.from_bytes(record[14:16], "little") == 955
    assert int.from_bytes(record[16:20], "little") == 16
print("RECORDS_OK", flush=True)
