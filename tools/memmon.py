"""Run a command and report peak RSS (VmHWM) of the process and its descendants.
Usage: python3 memmon.py <label> -- <cmd> [args...]
"""

import os
import subprocess
import sys
import time


def read_hwm(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmHWM:"):
                    return int(line.split()[1])  # kB (peak RSS)
    except OSError:
        pass
    return 0


def read_name(pid):
    try:
        with open(f"/proc/{pid}/comm") as f:
            return f.read().strip()
    except OSError:
        return "?"


def descendants(root):
    children = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/status") as f:
                ppid = next(int(l.split()[1]) for l in f if l.startswith("PPid:"))
            children.setdefault(ppid, []).append(int(entry))
        except (OSError, StopIteration):
            continue
    out, stack = [], [root]
    while stack:
        p = stack.pop()
        for c in children.get(p, []):
            out.append(c)
            stack.append(c)
    return out


def main():
    label = sys.argv[1]
    cmd = sys.argv[sys.argv.index("--") + 1:]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak = {}  # pid -> (name, max hwm kB)
    while proc.poll() is None:
        for pid in [proc.pid] + descendants(proc.pid):
            hwm = read_hwm(pid)
            if hwm:
                name = read_name(pid)
                prev = peak.get(pid, (name, 0))[1]
                peak[pid] = (name, max(prev, hwm))
        time.sleep(0.003)
    proc.wait()

    main_pid = peak.get(proc.pid, ("main", 0))
    children_peak = [(n, k) for pid, (n, k) in peak.items() if pid != proc.pid]
    chrome = max((k for n, k in children_peak if "chrome" in n or "headless" in n), default=0)
    total = main_pid[1] + sum(k for _, k in children_peak)
    print(
        f"{label:26s} process({main_pid[0]})={main_pid[1] / 1024:6.0f}MB  "
        f"chrome={chrome / 1024:6.0f}MB  tree_total={total / 1024:6.0f}MB"
    )


if __name__ == "__main__":
    main()
