#!/usr/bin/env python3
"""Linux-only, bounded ancestry/reaping experiment; not a supervisor test."""

import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import select
import signal
import time


def stat(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
    except (FileNotFoundError, ProcessLookupError):
        return None
    # comm can contain spaces and parentheses. Field 22 is starttime.
    tail = raw[raw.rfind(")") + 2:].split()
    return {"pid": pid, "ppid": int(tail[1]), "state": tail[0],
            "starttime_ticks": int(tail[19])}


def stamp():
    return {"utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "monotonic_ns": time.monotonic_ns()}


def worker(level, write_fd):
    """Explicit waitpid on TERM; idle parents do not reap before abrupt KILL."""
    child = None
    # Also bound living fixtures if the observer is interrupted or dies.
    signal.signal(signal.SIGALRM, signal.SIG_DFL)
    signal.alarm(8)

    def terminate(_signum, _frame):
        if child is not None:
            os.kill(child, signal.SIGTERM)
            os.waitpid(child, 0)
        os._exit(0)

    signal.signal(signal.SIGTERM, terminate)
    if level < 2:
        child = os.fork()
        if child == 0:
            worker(level + 1, write_fd)
            os._exit(0)
    # Records are small atomic pipe writes; only these three processes exist.
    record = {"level": level, **stat(os.getpid()), **stamp()}
    os.write(write_fd, (json.dumps(record) + "\n").encode())
    os.close(write_fd)
    while True:
        signal.pause()


def observe(records):
    rows = []
    for identity in records:
        current = stat(identity["pid"])
        same = current is not None and current["starttime_ticks"] == identity["starttime_ticks"]
        try:
            os.kill(identity["pid"], 0)
            probe = "exists"
        except ProcessLookupError:
            probe = "ESRCH"
        except PermissionError:
            probe = "EPERM"
        if current is None:
            kind = "gone"
        elif not same:
            kind = "pid-reused"
        elif current["state"] == "Z":
            kind = "ppid1-zombie" if current["ppid"] == 1 else "parent-owned-zombie"
        else:
            kind = "live"
        rows.append({"owned_identity": identity, "current": current,
                     "same_identity": same, "kill_0": probe, "classification": kind})
    return {**stamp(), "processes": rows}


def send(identity, pidfd, signum, events):
    current = stat(identity["pid"])
    if current is None or current["starttime_ticks"] != identity["starttime_ticks"]:
        return
    if current["state"] == "Z":
        return  # Zombies cannot be killed; their adopting parent must reap.
    try:
        signal.pidfd_send_signal(pidfd, signum)
        events.append({**stamp(), "pid": identity["pid"], "signal": signal.Signals(signum).name})
    except ProcessLookupError:
        pass


def run_case(mode):
    read_fd, write_fd = os.pipe()
    root = os.fork()
    if root == 0:
        os.close(read_fd)
        worker(0, write_fd)
        os._exit(0)
    os.close(write_fd)
    records, handles, events, snapshots = [], {}, [], []
    root_identity = stat(root)
    root_handle = os.pidfd_open(root)
    handles[root] = root_handle
    root_reaped = False
    result = {"mode": mode, "events": events, "snapshots": snapshots}
    try:
        pending = b""
        deadline = time.monotonic() + 3
        while len(records) < 3:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([read_fd], [], [], max(0, remaining))[0]:
                raise RuntimeError("three-process readiness deadline expired")
            chunk = os.read(read_fd, 4096)
            if not chunk:
                raise RuntimeError("readiness pipe closed before three identities")
            pending += chunk
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                identity = json.loads(line)
                records.append(identity)
                if identity["pid"] not in handles:
                    handles[identity["pid"]] = os.pidfd_open(identity["pid"])
        records.sort(key=lambda row: row["level"])
        expected_ppids = [os.getpid(), root, records[1]["pid"]]
        if [r["level"] for r in records] != [0, 1, 2] or records[0]["pid"] != root:
            raise RuntimeError("unexpected fixture identities")
        for identity, expected in zip(records, expected_ppids):
            current = stat(identity["pid"])
            if current is None or current["ppid"] != expected or current["starttime_ticks"] != identity["starttime_ticks"]:
                raise RuntimeError("fixture ancestry/identity changed before signals")
        snapshots.append(observe(records))
        if mode == "cooperative":
            send(records[0], handles[root], signal.SIGTERM, events)
        else:
            for identity in reversed(records):
                send(identity, handles[identity["pid"]], signal.SIGKILL, events)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if not root_reaped:
                root_reaped = os.waitpid(root, os.WNOHANG)[0] == root
            snapshot = observe(records)
            snapshots.append(snapshot)
            if all(r["classification"] in ("gone", "pid-reused") for r in snapshot["processes"]):
                break
            time.sleep(0.05)
        result["root_reaped"] = root_reaped
    except Exception as error:
        result["error"] = str(error)
    finally:
        # Only this fixture's verified ancestry is eligible; no group signals.
        owned = {r["pid"]: r for r in records}
        owned.setdefault(root, root_identity)
        # Readiness can fail before all pipe records are consumed. Discover only
        # descendants of the still-owned root before signalling any parent.
        current_root = stat(root)
        if current_root and current_root["starttime_ticks"] == root_identity["starttime_ticks"]:
            table = []
            for entry in Path("/proc").iterdir():
                if entry.name.isdecimal():
                    row = stat(int(entry.name))
                    if row:
                        table.append(row)
            for _ in range(2):
                for row in table:
                    if row["ppid"] in owned and row["pid"] not in owned:
                        row["level"] = owned[row["ppid"]].get("level", 0) + 1
                        try:
                            handles[row["pid"]] = os.pidfd_open(row["pid"])
                        except ProcessLookupError:
                            continue
                        owned[row["pid"]] = row
        records[:] = sorted(owned.values(), key=lambda row: row.get("level", 0))
        for identity in sorted(owned.values(), key=lambda row: row.get("level", 0), reverse=True):
            if identity["pid"] in handles:
                send(identity, handles[identity["pid"]], signal.SIGKILL, events)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if not root_reaped:
                root_reaped = os.waitpid(root, os.WNOHANG)[0] == root
            snapshot = observe(records)
            snapshots.append(snapshot)
            if root_reaped and not any(r["classification"] == "live" for r in snapshot["processes"]):
                break
            time.sleep(0.05)
        snapshots.append(observe(records))
        result["root_reaped"] = root_reaped
        for handle in handles.values():
            os.close(handle)
        os.close(read_fd)
    final = snapshots[-1]["processes"]
    result["live_survivors"] = [r["owned_identity"]["pid"] for r in final if r["classification"] == "live"]
    result["ppid1_zombies"] = [r["owned_identity"]["pid"] for r in final if r["classification"] == "ppid1-zombie"]
    result["all_original_identities_gone"] = len(final) == 3 and all(r["classification"] in ("gone", "pid-reused") for r in final)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", required=True,
                        help="create six fixture processes; may leave at most two PID1 zombies")
    parser.parse_args()
    if platform.system() != "Linux" or not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        parser.error("Linux with Python pidfd_open/pidfd_send_signal is required; no unsafe fallback")
    # Check kernel support before creating a fixture.
    handle = os.pidfd_open(os.getpid())
    os.close(handle)
    report = {"schema": 1, "started": stamp(), "kernel": platform.release(),
              "python": platform.python_version(),
              "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
              "pid1": stat(1), "pid1_cmdline": Path("/proc/1/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace"),
              "bounds": {"cases": 2, "processes_per_case": 3, "maximum_orphan_zombies": 2,
                         "readiness_seconds": 3, "observation_seconds": 2, "cleanup_seconds": 2,
                         "worker_lifetime_seconds": 8}}
    control = run_case("cooperative")
    report["cases"] = [control]
    # If the control cannot reap, do not compound its footprint with another tree.
    if control.get("error") or not control["root_reaped"] or not control["all_original_identities_gone"]:
        report["verdict"] = "control-failed; abrupt case not run"
        status = 1
    else:
        abrupt = run_case("bottom-up-kill")
        report["cases"].append(abrupt)
        if abrupt.get("error") or abrupt["live_survivors"] or not abrupt["root_reaped"]:
            report["verdict"] = "diagnostic-failed"
            status = 1
        elif abrupt["ppid1_zombies"]:
            report["verdict"] = "orphan-zombies-observed"
            status = 0
        elif abrupt["all_original_identities_gone"]:
            report["verdict"] = "original-identities-gone-within-window"
            status = 0
        else:
            report["verdict"] = "inconclusive-process-state"
            status = 1
    report["finished"] = stamp()
    print(json.dumps(report, indent=2))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
