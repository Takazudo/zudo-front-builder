"""Absolute WSL wrapper supervision; the 60-second reserve is inside total."""

import json
import os
from pathlib import Path
import signal
import subprocess
import time

FINALIZATION_NS = 60_000_000_000
KILL_RESERVE_NS = 5_000_000_000


def bounded_bytes(path, limit):
    with Path(path).open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("outer input cap: " + str(path))
    return data


def preserve_hold(paths, state):
    # Create only new evidence. A pre-existing machine admission marker belongs
    # to its original owner even if its recorded state is surprising.
    payload = (json.dumps(state, separators=(",", ":")) + "\n").encode()
    results = []
    for path in paths:
        try:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_CLOEXEC, 0o600)
        except FileExistsError:
            results.append(dict(path=str(path), state="existing-preserved"))
            continue
        try:
            with os.fdopen(fd, "wb", closefd=False) as out:
                out.write(payload)
                out.flush()
                os.fsync(fd)
            parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
            try:
                os.fsync(parent)
            finally:
                os.close(parent)
        finally:
            os.close(fd)
        results.append(dict(path=str(path), state="created"))
    return results


def finish_cli(cli, end_ns, pidfd, controller, record, *, now=time.monotonic_ns):
    """Always observe direct-child exit or return an unresolved ownership row.

    Popen still owns an unreaped child when pidfd acquisition failed. Its
    send_signal/poll/wait operations keep ownership of that original child.
    A qualified controller tuple is optional and can only be signalled via
    its retained pidfd (never by reacquiring a numeric PID).
    """
    result = cli.poll()
    if result is not None:
        return result, None
    def send(sig):
        if cli.poll() is not None:
            return
        try:
            if pidfd is None:
                cli.send_signal(sig)
            else:
                signal.pidfd_send_signal(pidfd, sig)
        except ProcessLookupError:
            pass
        record(dict(stage="signal",target="supervisor",pid=cli.pid,signal=int(sig)))
    send(signal.SIGTERM)
    left=end_ns-KILL_RESERVE_NS-now()
    if left>0:
        try:
            return cli.wait(timeout=left/1e9),None
        except subprocess.TimeoutExpired:
            pass
    if controller is not None:
        try:
            signal.pidfd_send_signal(controller[0],signal.SIGKILL)
            record(dict(stage="signal",target="controller",birth=controller[1],signal=int(signal.SIGKILL)))
        except ProcessLookupError:
            pass
    send(signal.SIGKILL)
    left=end_ns-now()
    if left>0:
        try:
            result=cli.wait(timeout=left/1e9)
        except subprocess.TimeoutExpired:
            pass
    if result is None:
        result=cli.poll()
    # A forced stop never certifies closure of a server/browser or its children.
    return result,dict(supervisorPid=cli.pid,controller=controller[1] if controller else None,
                       supervisorReaped=result is not None,reason="forced or unconfirmed outer shutdown")


def disposition(rc, captured, sent, terminal):
    if not captured:
        raise ValueError("independent browser inventory absent")
    if terminal=="prewrite-refused" and rc!=0:
        return "normal-refusal"
    if sent and terminal=="decision-only" and rc!=0:
        return "verified-no-write"
    if sent and terminal=="body" and rc==0:
        return "verified-body"
    raise ValueError("missing or conflicting normal controller/outer outcome")
