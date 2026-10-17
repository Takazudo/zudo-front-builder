"""Isolated POSIX drain proposal. No production checker imports this module.

Proposal: execution T (60s) plus ONE cleanup C (1s), measured after Popen.
Not a hard API-entry ceiling, tree cleanup guarantee, or bounded-memory collector.
A caller MUST retain/reap result['pending_child'] and dispose of every handle
in result['pending_streams'] on cleanup failure. Cancellation
propagates with the same diagnostic attached as exception.watchdog_cleanup.
"""
import math
import os
import subprocess
import time


def run(argv, wire=b"", *, timeout=60, cleanup_timeout=1,
        _popen=subprocess.Popen, _clock=time.monotonic):
    if os.name != "posix":
        raise ValueError("this isolated proposal supports POSIX only")
    for value in (timeout, cleanup_timeout):
        if isinstance(value, bool) or not math.isfinite(value) or value <= 0:
            raise ValueError("budgets must be finite positive seconds")
    if not isinstance(wire, bytes):
        raise TypeError("wire must be bytes")
    # No Popen context manager: its exit path may wait without a deadline.
    child = _popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                   stderr=subprocess.PIPE)
    start = _clock()
    total = start + timeout + cleanup_timeout
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
            remaining = max(0, deadline - _clock())
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
        cleanup(min(total, _clock() + cleanup_timeout))
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
