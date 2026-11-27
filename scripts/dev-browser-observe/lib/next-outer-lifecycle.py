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


class OwnershipMarker:
    """A positive durable ownership claim precedes Popen and stays on failure."""
    def __init__(self, path, initial):
        self.path = path
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_APPEND | os.O_CLOEXEC, 0o600)
        try:
            self.identity = os.fstat(self.fd)
            self.append(initial)
            self.sync_parent()
        except BaseException:
            os.close(self.fd)
            raise

    def sync_parent(self):
        fd = os.open(self.path.parent, os.O_DIRECTORY | os.O_RDONLY | os.O_CLOEXEC)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

    def append(self, row):
        data = (json.dumps(row, separators=(",", ":")) + "\n").encode()
        if len(data) > 8192:
            raise ValueError("outer ownership record cap")
        with os.fdopen(self.fd, "ab", closefd=False) as stream:
            stream.write(data)
            stream.flush()
            os.fsync(self.fd)

    def release(self):
        current = self.path.stat(follow_symlinks=False)
        if (current.st_dev,current.st_ino) != (self.identity.st_dev,self.identity.st_ino):
            raise ValueError("outer ownership marker replaced")
        self.path.unlink()
        self.sync_parent()

    def close(self):
        os.close(self.fd)


def preserve_hold(paths, state):
    # Each destination is attempted independently. The pre-launch marker
    # already exists even when every output filesystem subsequently goes bad.
    results = []
    for path in paths:
        try:
            marker = OwnershipMarker(path, state)
            marker.close()
            outcome = dict(state="created")
        except FileExistsError as error:
            if os.path.lexists(path):
                outcome = dict(state="existing-preserved")
            else:
                outcome = dict(state="write-failed",error=repr(error))
        except OSError as error:
            outcome = dict(state="write-failed", error=repr(error))
        results.append(dict(path=str(path), **outcome))
    return results


def finish_cli(cli, end_ns, pidfd, controller, record, *, now=time.monotonic_ns):
    """Signal only retained identities and charge all waiting to the absolute end."""
    end_ns = min(end_ns, now() + FINALIZATION_NS)
    problems = []
    def attempt(stage, operation):
        try:
            return operation()
        except Exception as error:
            problems.append(dict(stage=stage, error=repr(error)))
            return None

    result = attempt("poll", cli.poll)
    if result is not None:
        return result, None

    def send_child(sig):
        if attempt("pre-signal poll", cli.poll) is not None:
            return
        try:
            if pidfd is None:
                # Popen retains its direct unreaped child even when pidfd_open failed.
                cli.send_signal(sig)
            else:
                signal.pidfd_send_signal(pidfd, sig)
        except ProcessLookupError:
            pass
        except OSError as error:
            problems.append(dict(stage="supervisor signal", error=repr(error)))
        attempt("signal journal", lambda:record(dict(stage="signal", target="supervisor",
                                                    pid=cli.pid, signal=int(sig))))

    def wait_until(limit):
        left = limit - now()
        if left > 0:
            try:
                return cli.wait(timeout=left/1e9)
            except subprocess.TimeoutExpired:
                pass
            except OSError as error:
                problems.append(dict(stage="supervisor wait", error=repr(error)))
        return attempt("post-wait poll", cli.poll)

    send_child(signal.SIGTERM)
    result = wait_until(end_ns - KILL_RESERVE_NS)
    if result is not None and not problems:
        return result, None
    if result is None:
        if controller is not None:
            try:
                signal.pidfd_send_signal(controller[0],signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                problems.append(dict(stage="controller signal",error=repr(error)))
            attempt("controller signal journal", lambda:record(dict(
                stage="signal",target="controller",birth=controller[1],signal=int(signal.SIGKILL))))
        send_child(signal.SIGKILL)
        result = wait_until(end_ns)
    return result,dict(supervisorPid=cli.pid,controller=controller[1] if controller else None,
                       supervisorReaped=result is not None,reason="forced or unconfirmed outer shutdown",
                       problems=problems)


def disposition(rc, captured, sent, terminal):
    if not captured:
        raise ValueError("independent browser inventory absent")
    if terminal=="prewrite-refused" and rc!=0:
        return "normal-refusal"
    if sent and terminal=="decision-only" and rc!=0:
        return "verified-no-write"
    if sent and terminal=="body" and rc==0:
        return "verified-body"
    # A predeclared stale/timeout classification with exact restoration: the
    # controller's declared classify stop exits 1 (EXIT.stopped).
    if sent and terminal=="body-classified" and rc==1:
        return "verified-body-classified"
    raise ValueError("missing or conflicting normal controller/outer outcome")
