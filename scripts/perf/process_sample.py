"""Sample a Windows process using stdlib only. CPU is percent of one logical core."""
import ctypes
from ctypes import wintypes
import json
import sys
import time


def sample(pid, seconds=3):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4

    class Memory(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in (
                "peak_ws", "ws", "peak_paged", "paged", "peak_nonpaged",
                "nonpaged", "pagefile", "peak_pagefile", "private",
            )
        ]

    memory_info = ctypes.WinDLL("psapi").GetProcessMemoryInfo
    memory_info.argtypes = [wintypes.HANDLE, ctypes.POINTER(Memory), wintypes.DWORD]
    handle = kernel.OpenProcess(0x410, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())

    def cpu():
        values = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *[ctypes.byref(v) for v in values]):
            raise ctypes.WinError(ctypes.get_last_error())
        return sum((v.dwHighDateTime << 32) | v.dwLowDateTime for v in values[2:]) / 10_000_000

    try:
        before, start = cpu(), time.perf_counter()
        time.sleep(seconds)
        used, elapsed = cpu() - before, time.perf_counter() - start
        memory = Memory()
        memory.cb = ctypes.sizeof(memory)
        if not memory_info(handle, ctypes.byref(memory), memory.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return {"seconds": round(elapsed, 3), "cpu_one_core_percent": round(used / elapsed * 100, 2),
                "working_set_mib": round(memory.ws / 1048576, 2), "private_mib": round(memory.private / 1048576, 2)}
    finally:
        kernel.CloseHandle(handle)


if __name__ == "__main__":
    print(json.dumps(sample(int(sys.argv[1]), float(sys.argv[2]) if len(sys.argv) > 2 else 3)))
