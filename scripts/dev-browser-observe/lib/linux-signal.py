"""Signal a pinned Linux process (or its leader-owned group) through a pidfd."""

import json
import os
import signal
import sys
from pathlib import Path


def deliver(pid, pgid, boot_id, ticks, sig, group):
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return {"status": "gone"}
    try:
        try:
            stat = Path(f"/proc/{pid}/stat").read_bytes()
        except FileNotFoundError:
            return {"status": "gone"}
        fields = stat[stat.rindex(b")") + 1:].split()
        actual = {
            "pid": pid,
            "pgid": int(fields[2]),
            "birthVersion": "linux-proc-v1",
            "startTicks": fields[19].decode("ascii"),
            "bootId": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
        }
        if (actual["bootId"] != boot_id or actual["startTicks"] != ticks
                or actual["pgid"] != pgid or (group and pid != pgid)):
            return {"status": "mismatch", "actual": actual}
        # Linux UAPI PIDFD_SIGNAL_PROCESS_GROUP (since 6.9). Unsupported flags
        # fail closed. The descriptor cannot retarget a recycled PID/PGID.
        try:
            signal.pidfd_send_signal(fd, signal.Signals[sig], None, 4 if group else 0)
        except ProcessLookupError:
            return {"status": "gone"}
        return {"status": "sent"}
    finally:
        os.close(fd)


if __name__ == "__main__":
    print(json.dumps(deliver(int(sys.argv[1]), int(sys.argv[2]), *sys.argv[3:6],
                             sys.argv[6] == "group")))
