"""Bounded POSIX runner watchdog with a post-kill drain (#4289, option A).

Execution T (60s) plus ONE shared cleanup C (1s), both measured after Popen
returns: kill, drain and reap of the direct child share D_total = t0 + T + C.
Not an API-entry ceiling, descendant cleanup, or bounded-memory collector.
Live handles the drain cannot finish are returned in result['pending_child']
and result['pending_streams']; callers hand them to a PendingRegistry
(WATCHDOG-OWNERSHIP-4350.md section 5). Cancellation propagates with the same
diagnostic attached as exception.watchdog_cleanup. An interrupt inside Popen
itself, before the child handle exists, is not covered: there is no signal mask.
"""
import math
import os
import subprocess
import time


def run(argv, wire=b"", *, timeout=60, cleanup_timeout=1, _popen=None, _clock=time.monotonic):
    if os.name != "posix":
        raise ValueError("the bounded drain supports POSIX only")
    for value in (timeout, cleanup_timeout):
        if isinstance(value, bool) or not math.isfinite(value) or value <= 0:
            raise ValueError("budgets must be finite positive seconds")
    if not isinstance(wire, bytes):
        raise TypeError("wire must be bytes")
    result = dict(passed=False, exit_code=None, stdout=b"", stderr=b"",
                  watchdog=False, drain_complete=False, cleanup_errors=[],
                  cleanup_exit_code=None, pending_child=None, pending_streams=[],
                  execution_seconds=timeout, cleanup_seconds=cleanup_timeout)

    def snapshot(stdout, stderr):
        # communicate retry snapshots are cumulative, not incremental tails.
        for name, value in (("stdout", stdout), ("stderr", stderr)):
            if value is not None:
                result[name] = value

    cleanup_interrupt = None

    def error(phase, exc):
        nonlocal cleanup_interrupt
        if not isinstance(exc, Exception) and cleanup_interrupt is None:
            cleanup_interrupt = exc
        result["cleanup_errors"].append(f"{phase}: {type(exc).__name__}: {exc}")

    def dispose(names=("stdin", "stdout", "stderr")):
        for name in names:
            stream = getattr(child, name)
            if stream is None or stream.closed:
                continue
            if name == "stdin":
                try:
                    os.set_blocking(stream.fileno(), False)
                except BaseException as exc:
                    # Do not risk a blocking flush; transfer this open handle.
                    error("nonblocking stdin", exc)
                    result["pending_streams"].append(stream)
                    continue
            try:
                stream.close()
            except BaseException as exc:
                error(f"close {name}", exc)
            if not stream.closed:
                result["pending_streams"].append(stream)

    def cleanup(deadline):
        try:
            try:
                child.kill()
            except ProcessLookupError:
                pass
            except BaseException as exc:
                error("kill", exc)
            dispose(("stdin",))
            if child.stdin is not None and child.stdin.closed:
                child.stdin = None
            remaining = 0 if deadline is None else max(0, deadline - _clock())
            if result["pending_streams"]:
                result["cleanup_errors"].append("drain refused with unresolved stdin ownership")
            elif remaining:
                try:
                    out, err = child.communicate(input=None, timeout=remaining)
                    snapshot(out, err)
                    result["drain_complete"] = True
                except subprocess.TimeoutExpired as exc:
                    snapshot(exc.stdout, exc.stderr)
                    error("drain deadline", exc)
                except BaseException as exc:
                    error("drain", exc)
            elif deadline is None:
                result["cleanup_errors"].append("cleanup deadline unavailable")
            else:
                result["cleanup_errors"].append("cleanup deadline exhausted")
        except BaseException as exc:
            error("cleanup", exc)
        finally:
            dispose(("stdout", "stderr"))
            try:
                result["cleanup_exit_code"] = child.poll()
            except BaseException as exc:
                error("poll", exc)
            if result["cleanup_exit_code"] is None:
                result["pending_child"] = child
                result["cleanup_errors"].append("direct child ownership remains with caller")

    # No Popen context manager: its exit path may wait without a deadline.
    # Looked up per call so a test can substitute subprocess.Popen.
    child = (_popen or subprocess.Popen)(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE)
    try:
        start = _clock()
        total = start + timeout + cleanup_timeout
    except BaseException as exc:
        # U1: an interrupt before the deadline exists still kills and attaches the child.
        cleanup(None)
        exc.watchdog_cleanup = result
        raise
    try:
        out, err = child.communicate(input=wire, timeout=max(0, start + timeout - _clock()))
    except subprocess.TimeoutExpired as exc:
        result["watchdog"] = True
        snapshot(exc.stdout, exc.stderr)
        cleanup(total)
        if cleanup_interrupt is not None:
            cleanup_interrupt.watchdog_cleanup = result
            raise cleanup_interrupt
    except BaseException as exc:
        # Early cancellation gets C, not the unused normal execution allowance.
        try:
            deadline = min(total, _clock() + cleanup_timeout)
        except BaseException as clock_exc:
            error("clock", clock_exc)
            deadline = None
        cleanup(deadline)
        exc.watchdog_cleanup = result
        raise
    else:
        snapshot(out, err)
        result["exit_code"] = child.returncode
        result["passed"] = child.returncode == 0
        result["drain_complete"] = True
        dispose()
        if result["cleanup_errors"] or result["pending_streams"]:
            result["passed"] = False
        if cleanup_interrupt is not None:
            cleanup_interrupt.watchdog_cleanup = result
            raise cleanup_interrupt
    return result


class PendingRegistry:
    """Process-lifetime owner of live handles a drain could not finish (U3/U4).

    It adds no wait budget: `release` only polls and closes once, and reports a
    child that is still alive as released to the OS (init/launchd reaps it).
    """

    def __init__(self):
        self.children, self.streams = [], []

    def project(self, result):
        """JSON-safe copy without stream bytes; live handles move into the registry."""
        view = {k: v for k, v in result.items()
                if k not in ("stdout", "stderr", "pending_child", "pending_streams")}
        child = result.get("pending_child")
        streams = list(result.get("pending_streams") or [])
        view["pending_child_pid"] = None if child is None else child.pid
        view["pending_stream_count"] = len(streams)
        if child is not None:
            self.children.append(child)
        self.streams.extend(streams)
        return view

    def release(self):
        released = []
        for child in self.children:
            try:
                code = child.poll()
            except Exception as error:  # Record, never wait; the state stays unknown.
                code = f"{type(error).__name__}: {error}"
            released.append({"pid": child.pid, "exit_code": code,
                             "released_to_os": not isinstance(code, int)})
        for stream in self.streams:
            # Close only without blocking: a buffered flush to a stalled reader could hang.
            try:
                os.set_blocking(stream.fileno(), False)
                stream.close()
            except Exception:
                pass
            if not stream.closed:
                released.append({"stream": repr(stream), "released_to_os": True})
        self.children, self.streams = [], []
        return released
