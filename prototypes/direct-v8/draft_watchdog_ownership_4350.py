"""Draft tests and adapter sketch for #4350 (cleanup ownership under #4289 E001-a).

DRAFT ONLY. Not discovered by `test_*.py`, not imported by any checker, and not
production acceptance. It targets the unmerged #4293 prototype at 509113ae:

    git show 509113ae:prototypes/direct-v8/watchdog-drain-prototype.py > /tmp/p.py
    ZFB_DRAIN_PROTOTYPE=/tmp/p.py python3 -I -B -W error::ResourceWarning \
        prototypes/direct-v8/draft_watchdog_ownership_4350.py

Without ZFB_DRAIN_PROTOTYPE every prototype test is skipped, not passed.
`expectedFailure` marks a gap the design names (U1, U7); it must flip to a plain
pass once an approved adapter closes it. No deadline here is larger than the
prototype's own, no test waits on the original #4292 Mac race, and real-child
fixtures carry their own alarms.
"""
import base64
import ctypes
import importlib.util
import io
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
PROTOTYPE = os.environ.get("ZFB_DRAIN_PROTOTYPE")


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


proposal = load("drain_proposal", PROTOTYPE) if PROTOTYPE else None


# --- Adapter sketch (design section 5). Not wired into runner-protocol.py. ---

class PendingRegistry:
    """Process-lifetime owner of live handles the drain could not finish.

    It adds no wait budget: `release` only polls and closes, and reports any
    child still alive as released to the OS (init/launchd/subreaper reaps it).
    """

    def __init__(self):
        self.children, self.streams = [], []

    def project(self, result):
        """JSON-safe copy of a drain result; live handles move into the registry."""
        view = {k: v for k, v in result.items() if k not in ("pending_child", "pending_streams")}
        for name in ("stdout", "stderr"):
            # Same lossless rule as runner-protocol.py save_stream.
            data = view.pop(name)
            try:
                view[name] = data.decode("utf-8")
            except UnicodeDecodeError:
                view[name] = data.decode("utf-8", errors="replace")
                view[name + "_base64"] = base64.b64encode(data).decode("ascii")
        child = result.get("pending_child")
        view["pending_child_pid"] = None if child is None else child.pid
        view["pending_stream_count"] = len(result.get("pending_streams") or [])
        if child is not None:
            self.children.append(child)
        self.streams.extend(result.get("pending_streams") or [])
        return view

    def release(self):
        released = []
        for child in self.children:
            try:
                code = child.poll()
            except Exception as error:  # Record, never wait.
                code = f"{type(error).__name__}: {error}"
            released.append({"pid": child.pid, "exit_code": code,
                             "released_to_os": code is None})
        for stream in self.streams:
            try:
                stream.close()
            except Exception:
                pass
        self.children, self.streams = [], []
        return released


class FakeChild:
    pid = 4242

    def __init__(self, actions, clock, *, poll_results=(-9,)):
        self.actions, self.clock = iter(actions), clock
        self.poll_results = list(poll_results)
        self.returncode = None
        self.stdin, self.stdout, self.stderr = (io.BytesIO() for _ in range(3))
        self.stdin.close()
        self.killed = 0

    def communicate(self, input=None, timeout=None):
        advance, answer = next(self.actions)
        self.clock[0] += advance
        if isinstance(answer, BaseException):
            raise answer
        return answer

    def kill(self):
        self.killed += 1

    def poll(self):
        return self.poll_results.pop(0) if len(self.poll_results) > 1 else self.poll_results[0]


def fake(actions, **kwargs):
    clock = [0.0]
    child = FakeChild(actions, clock, **kwargs)
    return child, (lambda *a, **k: child), (lambda: clock[0])


@unittest.skipUnless(proposal, "set ZFB_DRAIN_PROTOTYPE to #4293's watchdog-drain-prototype.py")
class OwnershipDraft(unittest.TestCase):
    def test_u3_raw_result_is_not_json_and_projection_moves_ownership(self):
        child, popen, clock = fake([(60, subprocess.TimeoutExpired("x", 60, output=b"p")),
                                    (1, subprocess.TimeoutExpired("x", 1))], poll_results=(None,))
        result = proposal.run(["x"], _popen=popen, _clock=clock)
        # Negative control: checker mains json.dumps their report (check-config.py main).
        with self.assertRaises(TypeError):
            json.dumps(result)
        registry = PendingRegistry()
        view = registry.project(result)
        self.assertEqual(json.loads(json.dumps(view))["pending_child_pid"], 4242)
        self.assertIs(registry.children[0], child)
        self.assertEqual(registry.release(),
                         [{"pid": 4242, "exit_code": None, "released_to_os": True}])

    def test_u2_zero_remaining_hands_off_a_killed_child_without_any_wait(self):
        # First timeout is observed only after the shared total (scheduler stall).
        child, popen, clock = fake([(61.5, subprocess.TimeoutExpired("x", 60))],
                                   poll_results=(None, -9))
        result = proposal.run(["x"], _popen=popen, _clock=clock)
        self.assertEqual(child.killed, 1)
        self.assertIn("cleanup deadline exhausted", result["cleanup_errors"])
        self.assertIs(result["pending_child"], child)  # dying, not yet reaped
        registry = PendingRegistry()
        registry.project(result)
        # Delayed reap: the later non-blocking poll observes the exit; no new budget.
        self.assertEqual(registry.release(),
                         [{"pid": 4242, "exit_code": -9, "released_to_os": False}])

    @unittest.expectedFailure  # U1: fails on 509113ae; flips when spawn bookkeeping is guarded.
    def test_u1_interrupt_between_spawn_and_bookkeeping_still_attaches_child(self):
        child, popen, _ = fake([])

        def interrupted_clock():
            raise KeyboardInterrupt("between Popen and the deadline stamp")

        with self.assertRaises(KeyboardInterrupt) as caught:
            proposal.run(["x"], _popen=popen, _clock=interrupted_clock)
        self.assertEqual(child.killed, 1)
        self.assertIn("watchdog_cleanup", vars(caught.exception))

    def test_u8_non_posix_is_refused_so_an_adapter_needs_the_legacy_path(self):
        with mock.patch.object(proposal, "os", SimpleNamespace(name="nt")):
            with self.assertRaises(ValueError):
                proposal.run(["x"], _popen=lambda *a, **k: self.fail("must not spawn"))

    @unittest.skipUnless(sys.platform == "linux", "Linux subreaper fixture; Mac stays NOT RUN")
    def test_direct_child_exits_zero_while_grandchild_holds_stdout(self):
        libc = ctypes.CDLL(None, use_errno=True)
        old = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(old), 0, 0, 0), 0)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)
        readfd, writefd = os.pipe()
        children, descendant = [], None

        def spawn(*args, **kwargs):
            nonlocal descendant
            child = subprocess.Popen(*args, start_new_session=True, pass_fds=(writefd,), **kwargs)
            children.append(child)
            self.assertTrue(select.select([readfd], [], [], 5)[0], "fork fixture not ready")
            descendant = int(os.read(readfd, 64))
            return child

        # The direct child forks, reports the grandchild PID, then exits 0. The
        # grandchild keeps the inherited stdout/stderr write ends open.
        code = (f"import os,signal; signal.alarm(5); pid=os.fork(); "
                f"(os.write({writefd},str(pid).encode()), os._exit(0)) if pid else signal.pause()")
        start = time.monotonic()
        try:
            result = proposal.run([sys.executable, "-I", "-S", "-c", code], timeout=.2,
                                  cleanup_timeout=.1, _popen=spawn)
            self.assertLess(time.monotonic() - start, 3)
            self.assertTrue(result["watchdog"])          # EOF never came
            self.assertIsNone(result["exit_code"])       # watchdog keeps it null
            self.assertEqual(result["cleanup_exit_code"], 0)  # diagnostic only
            self.assertFalse(result["drain_complete"])
            self.assertIsNone(result["pending_child"])
            self.assertTrue(children[0].stdout.closed and children[0].stderr.closed)
        finally:
            os.close(readfd)
            os.close(writefd)
            for child in children:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=5)
            try:
                if descendant is not None:
                    self.assertEqual(os.waitpid(descendant, 0)[0], descendant)
            finally:
                self.assertEqual(libc.prctl(36, old.value, 0, 0, 0), 0)


class CheckerMainDraft(unittest.TestCase):
    """Checker-level ownership on the production checker source (no runner spawned)."""

    @unittest.expectedFailure  # U7: check-config main writes no report on cancellation.
    def test_u7_cancellation_still_writes_a_failed_report(self):
        checker = load("check_config_draft", HERE / "check-config.py")
        cancellation = KeyboardInterrupt("cancel")
        cancellation.watchdog_cleanup = {"watchdog": False, "pending_child_pid": None}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.json"
            argv = ["check-config.py", "--baseline", sys.executable, "--output", str(output)]
            with mock.patch.object(sys, "argv", argv), \
                    mock.patch.object(checker.protocol, "run_receipts", side_effect=cancellation):
                with self.assertRaises(KeyboardInterrupt):
                    checker.main()
            report = json.loads(output.read_text())
            self.assertFalse(report["passed"])
            self.assertIn("cancel", report["error"])


if __name__ == "__main__":
    unittest.main()
