"""Tests of an isolated proposal, not production watchdog qualification."""
import ctypes
import importlib.util
import io
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import time
import unittest

spec = importlib.util.spec_from_file_location("drain_proposal", Path(__file__).with_name("watchdog-drain-prototype.py"))
proposal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proposal)


class FakeChild:
    def __init__(self, actions, clock, *, kill_error=None, poll_result=-9):
        self.actions = iter(actions)
        self.clock = clock
        self.kill_error = kill_error
        self.poll_result = poll_result
        self.returncode = None
        self.stdin, self.stdout, self.stderr = (io.BytesIO() for _ in range(3))
        self.calls = []

    def communicate(self, input=None, timeout=None):
        self.calls.append((input, timeout))
        advance, answer = next(self.actions)
        self.clock[0] += advance
        if isinstance(answer, BaseException):
            raise answer
        self.returncode = self.poll_result
        return answer

    def kill(self):
        if self.kill_error:
            raise self.kill_error

    def poll(self):
        return self.poll_result


class DrainProposalTest(unittest.TestCase):
    def fake(self, actions, **kwargs):
        clock = [0.0]
        child = FakeChild(actions, clock, **kwargs)
        # Fake stdin has no OS fd; close it as communicate normally does.
        child.stdin.close()
        return child, lambda: clock[0]

    def test_shared_deadline_and_cumulative_snapshots(self):
        child, clock = self.fake([(60.25, subprocess.TimeoutExpired('x', 60, output=b'a')),
                                  (.5, (b'ab\xff', b'err\xfe'))])
        result = proposal.run(['x'], b'wire', _popen=lambda *a, **k: child, _clock=clock)
        self.assertEqual(child.calls, [(b'wire', 60), (None, .75)])
        self.assertEqual(result['stdout'], b'ab\xff')
        self.assertEqual(result['stderr'], b'err\xfe')
        self.assertTrue(result['watchdog'])
        self.assertFalse(result['passed'])
        self.assertIsNone(result['exit_code'])
        self.assertIsNone(result['pending_child'])

    def test_expired_total_never_starts_another_wait(self):
        child, clock = self.fake([(62, subprocess.TimeoutExpired('x', 60))])
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertEqual(len(child.calls), 1)
        self.assertFalse(result['drain_complete'])
        self.assertTrue(result['cleanup_errors'])

    def test_kill_failure_and_final_wait_without_snapshot_retains_public_bytes(self):
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60, output=b'known')),
                                  (1, subprocess.TimeoutExpired('x', 1))],
                                 kill_error=PermissionError('denied'), poll_result=None)
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertEqual(result['stdout'], b'known')
        self.assertIs(result['pending_child'], child)
        self.assertIsNone(result['exit_code'])
        self.assertFalse(result['drain_complete'])
        self.assertIn('kill: PermissionError', '\n'.join(result['cleanup_errors']))

    def test_early_cancellation_gets_only_cleanup_allowance_and_propagates(self):
        cancellation = KeyboardInterrupt('cancel')
        child, clock = self.fake([(2, cancellation), (.5, (b'late', b''))])
        with self.assertRaises(KeyboardInterrupt) as caught:
            proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertIs(caught.exception, cancellation)
        self.assertEqual(child.calls[1], (None, 1))
        self.assertEqual(cancellation.watchdog_cleanup['stdout'], b'late')

    def test_valid_receipt_after_timeout_cannot_rescue_failure(self):
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60)),
                                  (0, (b'ZFB_OUTER_CONFIG_RECEIPT {"ok":true}\n', b''))], poll_result=0)
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertFalse(result['passed'])
        self.assertIsNone(result['exit_code'])
        self.assertEqual(result['cleanup_exit_code'], 0)

    def test_cleanup_io_failure_does_not_erase_timeout_snapshot(self):
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60, stderr=b'known')),
                                  (0, OSError('read failed'))])
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertEqual(result['stderr'], b'known')
        self.assertFalse(result['drain_complete'])
        self.assertIn('drain: OSError', '\n'.join(result['cleanup_errors']))

    def test_close_and_poll_failures_are_explicit(self):
        class BadClose(io.BytesIO):
            def close(self):
                super().close()
                raise OSError('close failed')
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60)), (0, (b'', b''))])
        child.stdout = BadClose()
        def bad_poll():
            raise OSError('poll failed')
        child.poll = bad_poll
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertIs(result['pending_child'], child)
        self.assertIn('close stdout: OSError', '\n'.join(result['cleanup_errors']))
        self.assertIn('poll: OSError', '\n'.join(result['cleanup_errors']))

    def test_secondary_cleanup_interrupt_does_not_mask_original_cancellation(self):
        original, secondary = KeyboardInterrupt('first'), SystemExit('second')
        child, clock = self.fake([(2, original), (0, secondary)], poll_result=None)
        with self.assertRaises(KeyboardInterrupt) as caught:
            proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertIs(caught.exception, original)
        self.assertIs(original.watchdog_cleanup['pending_child'], child)
        self.assertIn('SystemExit', '\n'.join(original.watchdog_cleanup['cleanup_errors']))

    def test_cleanup_interrupt_after_timeout_propagates_with_ownership(self):
        interrupt = KeyboardInterrupt('cleanup')
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60)), (0, interrupt)], poll_result=None)
        with self.assertRaises(KeyboardInterrupt) as caught:
            proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertIs(caught.exception, interrupt)
        self.assertIs(interrupt.watchdog_cleanup['pending_child'], child)
        self.assertTrue(interrupt.watchdog_cleanup['watchdog'])

    def test_genuine_open_stream_failure_transfers_ownership_after_reaping(self):
        class RefusesClose(io.BytesIO):
            def close(self):
                raise OSError('still open')
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60)), (0, (b'', b''))])
        stream = child.stdout = RefusesClose()
        try:
            result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
            self.assertIsNone(result['pending_child'])
            self.assertEqual(result['pending_streams'], [stream])
        finally:
            io.BytesIO.close(stream)

    def test_nonblocking_setup_failure_retains_stdin_and_skips_retry(self):
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60))])
        stream = child.stdin = io.BytesIO()  # No fileno: safe close policy cannot be established.
        try:
            result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
            self.assertEqual(result['pending_streams'], [stream])
            self.assertEqual(len(child.calls), 1)
        finally:
            stream.close()

    def test_real_pre_output_timeout(self):
        result = proposal.run([sys.executable, '-I', '-S', '-c',
                               'import signal; signal.alarm(5); signal.pause()'],
                              timeout=.05, cleanup_timeout=.2)
        self.assertTrue(result['watchdog'])
        self.assertEqual((result['stdout'], result['stderr']), (b'', b''))
        self.assertIsNone(result['pending_child'])

    def test_real_late_bytes_are_drained_without_duplicate_prefix(self):
        readfd, writefd = os.pipe()
        children = []
        class ReadyChild(subprocess.Popen):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, pass_fds=(writefd,), **kwargs)
                children.append(self)
                self.first = True
            def communicate(self, input=None, timeout=None):
                if self.first:
                    self.first = False
                    if not select.select([readfd], [], [], 5)[0]:
                        raise AssertionError('fixture not ready')
                    if os.read(readfd, 1) != b'R':
                        raise AssertionError('missing readiness')
                    raise subprocess.TimeoutExpired(self.args, timeout, output=b'prefix')
                return super().communicate(input=input, timeout=timeout)
        try:
            code = f"import os,signal; signal.alarm(5); os.write(1,b'prefix\\xff'); os.write(2,b'err\\xfe'); os.write({writefd},b'R'); signal.pause()"
            result = proposal.run([sys.executable, '-I', '-S', '-c', code], timeout=1,
                                  cleanup_timeout=.2, _popen=ReadyChild)
            self.assertEqual(result['stdout'], b'prefix\xff')
            self.assertEqual(result['stderr'], b'err\xfe')
            self.assertFalse(result['passed'])
            self.assertIsNone(result['pending_child'])
        finally:
            os.close(readfd)
            os.close(writefd)
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux subreaper fixture; native Mac descendant acceptance remains separate')
    def test_real_grandchild_held_pipes_return_without_tree_claim(self):
        libc = ctypes.CDLL(None, use_errno=True)
        old_subreaper = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(old_subreaper), 0, 0, 0), 0)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)
        children = []
        descendant = None
        readfd, writefd = os.pipe()
        def spawn(*args, **kwargs):
            child = subprocess.Popen(*args, start_new_session=True,
                                     pass_fds=(writefd,), **kwargs)
            children.append(child)
            self.assertTrue(select.select([readfd], [], [], 5)[0], 'fork fixture not ready')
            nonlocal descendant
            descendant = int(os.read(readfd, 64))
            return child
        # Acknowledgment puts fork completion before the proposal's clock starts.
        # The test owns the process group; the proposal deliberately does not.
        code = (f'import os,signal; signal.alarm(5); pid=os.fork(); '
                f'signal.alarm(5); os.write({writefd},str(pid).encode()) if pid else None; signal.pause()')
        start = time.monotonic()
        try:
            result = proposal.run([sys.executable, '-I', '-S', '-c', code], timeout=.2,
                                  cleanup_timeout=.1, _popen=spawn)
            self.assertLess(time.monotonic() - start, 3)
            self.assertTrue(result['watchdog'])
            self.assertFalse(result['drain_complete'])
            self.assertIsNone(result['pending_child'])
            self.assertTrue(all(stream.closed for stream in (children[0].stdout, children[0].stderr)))
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
                    # Linux test process adopted the orphan; reap it explicitly.
                    self.assertEqual(os.waitpid(descendant, 0)[0], descendant)
            finally:
                self.assertEqual(libc.prctl(36, old_subreaper.value, 0, 0, 0), 0)

    def test_normal_success_and_nonzero_exit(self):
        for code in (0, 3):
            result = proposal.run([sys.executable, '-I', '-S', '-c', f"print('ok'); raise SystemExit({code})"])
            self.assertEqual(result['exit_code'], code)
            self.assertEqual(result['passed'], code == 0)
            self.assertEqual(result['stdout'], b'ok\n')


if __name__ == '__main__':
    unittest.main()
