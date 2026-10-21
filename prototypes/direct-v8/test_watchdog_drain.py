"""Bounded watchdog drain and its cleanup ownership (#4289 option A, #4350).

Run: python3 -I -B -W error::ResourceWarning -m unittest discover -s prototypes/direct-v8 -p 'test_watchdog*.py'
Fake clocks/processes pin the budget arithmetic; real-child fixtures carry
their own five-second alarms. The grandchild fixtures need a Linux subreaper
and are skipped, not passed, on macOS.
"""
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


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


proposal = load("watchdog_drain", HERE / "watchdog-drain.py")


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


@unittest.skipUnless(os.name == 'posix', 'the bounded drain is POSIX only')
class DrainTest(unittest.TestCase):
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


    def test_raw_result_is_not_json_and_projection_moves_ownership(self):
        # U3: live handles never reach a JSON report.
        child, clock = self.fake([(60, subprocess.TimeoutExpired('x', 60, output=b'p')),
                                  (1, subprocess.TimeoutExpired('x', 1))], poll_result=None)
        child.pid = 4242
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        with self.assertRaises(TypeError):
            json.dumps(result)
        registry = proposal.PendingRegistry()
        view = registry.project(result)
        self.assertEqual(json.loads(json.dumps(view))['pending_child_pid'], 4242)
        self.assertIs(registry.children[0], child)
        self.assertEqual(registry.release(), [{'pid': 4242, 'exit_code': None, 'released_to_os': True}])
        self.assertEqual(registry.release(), [])

    def test_zero_remaining_hands_off_a_killed_child_without_any_wait(self):
        # U2: the first timeout is observed only after the shared total.
        child, clock = self.fake([(61.5, subprocess.TimeoutExpired('x', 60))], poll_result=None)
        child.pid = 4242
        killed = []
        child.kill = lambda: killed.append(True)
        result = proposal.run(['x'], _popen=lambda *a, **k: child, _clock=clock)
        self.assertEqual((killed, len(child.calls)), ([True], 1))
        self.assertIn('cleanup deadline exhausted', result['cleanup_errors'])
        self.assertIs(result['pending_child'], child)
        registry = proposal.PendingRegistry()
        registry.project(result)
        child.poll_result = -9  # Delayed reap: a later poll sees the exit; no new budget.
        self.assertEqual(registry.release(), [{'pid': 4242, 'exit_code': -9, 'released_to_os': False}])

    def test_interrupt_between_spawn_and_bookkeeping_still_attaches_child(self):
        # U1: an interrupted deadline stamp still kills and attaches the child.
        child, _ = self.fake([])
        killed = []
        child.kill = lambda: killed.append(True)

        def interrupted_clock():
            raise KeyboardInterrupt('between Popen and the deadline stamp')

        with self.assertRaises(KeyboardInterrupt) as caught:
            proposal.run(['x'], _popen=lambda *a, **k: child, _clock=interrupted_clock)
        self.assertEqual(killed, [True])
        cleanup = caught.exception.watchdog_cleanup
        self.assertIn('cleanup deadline unavailable', cleanup['cleanup_errors'])
        self.assertEqual(child.calls, [])
        self.assertTrue(child.stdout.closed and child.stderr.closed)

    def test_non_posix_is_refused_before_spawning(self):
        # U8: run_receipts keeps the legacy path there.
        with mock.patch.object(proposal, 'os', SimpleNamespace(name='nt')):
            with self.assertRaises(ValueError):
                proposal.run(['x'], _popen=lambda *a, **k: self.fail('must not spawn'))

    def test_release_treats_an_unknown_state_and_an_open_stream_as_released(self):
        def bad_poll():
            raise OSError('poll failed')

        class RefusesClose(io.BytesIO):
            def close(self):
                raise OSError('still open')

        stream = RefusesClose()
        self.addCleanup(io.BytesIO.close, stream)
        registry = proposal.PendingRegistry()
        registry.project(dict(stdout=b'', stderr=b'', pending_streams=[stream],
                              pending_child=SimpleNamespace(pid=4242, poll=bad_poll)))
        self.assertEqual(registry.release(), [
            {'pid': 4242, 'exit_code': 'OSError: poll failed', 'released_to_os': True},
            {'stream': repr(stream), 'released_to_os': True}])

    def test_release_never_blocks_on_a_full_stdin_pipe(self):
        readfd, writefd = os.pipe()
        self.addCleanup(os.close, readfd)
        stream = open(writefd, 'wb', buffering=1 << 20)
        stream.write(b'x' * (256 << 10))  # Buffered, more than a pipe holds; close would flush it.
        registry = proposal.PendingRegistry()
        registry.project(dict(stdout=b'', stderr=b'', pending_child=None, pending_streams=[stream]))
        start = time.monotonic()
        self.assertEqual(registry.release(), [])
        self.assertLess(time.monotonic() - start, 3)
        self.assertTrue(stream.closed)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux subreaper fixture; Mac stays NOT RUN')
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
            self.assertTrue(select.select([readfd], [], [], 5)[0], 'fork fixture not ready')
            descendant = int(os.read(readfd, 64))
            return child

        # The direct child forks, reports the grandchild PID, then exits 0. The
        # grandchild keeps the inherited stdout/stderr write ends open.
        code = (f'import os,signal; signal.alarm(5); pid=os.fork(); '
                f'(os.write({writefd},str(pid).encode()), os._exit(0)) if pid else signal.pause()')
        start = time.monotonic()
        try:
            result = proposal.run([sys.executable, '-I', '-S', '-c', code], timeout=.2,
                                  cleanup_timeout=.1, _popen=spawn)
            self.assertLess(time.monotonic() - start, 3)
            self.assertTrue(result['watchdog'])
            self.assertIsNone(result['exit_code'])
            self.assertEqual(result['cleanup_exit_code'], 0)
            self.assertFalse(result['drain_complete'])
            self.assertIsNone(result['pending_child'])
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


class AdapterTest(unittest.TestCase):
    """runner-protocol.py and the checker mains around the drain."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.protocol = load('runner_protocol_test', HERE / 'runner-protocol.py')

    def runner(self, body):
        path = self.root / 'runner'
        path.write_text(f'#!{sys.executable} -S\n{body}\n')
        path.chmod(0o755)
        return path

    def test_non_posix_keeps_the_legacy_transport(self):
        completed = subprocess.CompletedProcess(['x'], 0, b'R {}\n', b'')
        with mock.patch.object(self.protocol, 'USE_DRAIN', False), \
                mock.patch.object(self.protocol.subprocess, 'run', return_value=completed) as run, \
                mock.patch.object(self.protocol.drain, 'run', side_effect=AssertionError('drain')):
            result = self.protocol.run_receipts(sys.executable, '', 'R ', 1)
        run.assert_called_once()
        self.assertTrue(result['passed'])
        self.assertNotIn('watchdog_cleanup', result)

    @unittest.skipUnless(os.name == 'posix', 'the bounded drain is POSIX only')
    def test_pending_child_fails_the_run_and_moves_to_the_registry(self):
        child = SimpleNamespace(pid=4242, poll=lambda: None)
        drained = dict(passed=False, exit_code=None, stdout=b'', stderr=b'', watchdog=True,
                       drain_complete=False, cleanup_errors=['direct child ownership remains with caller'],
                       cleanup_exit_code=None, pending_child=child, pending_streams=[],
                       execution_seconds=60, cleanup_seconds=1)
        with mock.patch.object(self.protocol.drain, 'run', return_value=drained):
            result = self.protocol.run_receipts(sys.executable, '', 'R ', 1)
        json.dumps(result)
        self.assertFalse(result['passed'])
        self.assertIsNone(result['exit_code'])
        self.assertEqual(result['watchdog_cleanup']['pending_child_pid'], 4242)
        self.assertIn('runner cleanup: direct child ownership remains with caller', result['failures'])
        self.assertEqual(self.protocol.PENDING.release(),
                         [{'pid': 4242, 'exit_code': None, 'released_to_os': True}])

    @unittest.skipUnless(os.name == 'posix', 'the bounded drain is POSIX only')
    def test_cancellation_propagates_with_a_json_safe_result(self):
        cancellation = KeyboardInterrupt('cancel')
        cancellation.watchdog_cleanup = dict(
            passed=False, exit_code=None, stdout=b'partial \xf0\x9f', stderr=b'', watchdog=False,
            drain_complete=True, cleanup_errors=[], cleanup_exit_code=-9, pending_child=None,
            pending_streams=[], execution_seconds=60, cleanup_seconds=1)
        with mock.patch.object(self.protocol.drain, 'run', side_effect=cancellation):
            with self.assertRaises(KeyboardInterrupt) as caught:
                self.protocol.run_receipts(sys.executable, '', 'R ', 1)
        result = caught.exception.watchdog_cleanup
        json.dumps(result)
        self.assertFalse(result['passed'])
        self.assertIn('runner interrupted: KeyboardInterrupt: cancel', result['failures'])
        self.assertEqual(result['stdout_base64'], 'cGFydGlhbCDwnw==')

    @unittest.skipUnless(os.name == 'posix', 'the bounded drain is POSIX only')
    def test_real_watchdog_reports_a_finished_drain(self):
        runner = self.runner('import os, signal, sys\nsys.stdin.buffer.read()\n'
                             'signal.alarm(5)\nsignal.pause()')
        result = self.protocol.run_receipts(runner, 'x\n', 'R ', 1, timeout=.3)
        self.assertFalse(result['passed'])
        self.assertIn('watchdog', result['watchdog'])
        cleanup = result['watchdog_cleanup']
        self.assertTrue(cleanup['drain_complete'])
        self.assertEqual(cleanup['cleanup_exit_code'], -signal.SIGKILL)
        self.assertIsNone(cleanup['pending_child_pid'])
        self.assertEqual(self.protocol.PENDING.release(), [])

    def check_main(self, checker, args):
        module = load('checker_main_test', HERE / checker)
        cancellation = KeyboardInterrupt('cancel')
        cancellation.watchdog_cleanup = {'passed': False, 'watchdog_cleanup': {'pending_child_pid': None}}
        output = self.root / 'report.json'
        argv = [checker, *args, '--output', str(output)]
        with mock.patch.object(sys, 'argv', argv), \
                mock.patch.object(module.protocol, 'run_receipts', side_effect=cancellation):
            with self.assertRaises(KeyboardInterrupt) as caught:
                module.main()
        self.assertIs(caught.exception, cancellation)
        return json.loads(output.read_text())

    def test_cancelled_config_check_still_writes_a_failed_report(self):
        # U7, on the production checker source (no runner spawned).
        report = self.check_main('check-config.py', ['--baseline', sys.executable])
        self.assertFalse(report['passed'])
        self.assertEqual(report['error'], 'interrupted: KeyboardInterrupt: cancel')
        self.assertFalse(report['watchdog_cleanup']['passed'])
        self.assertEqual(report['released_children'], [])

    def test_config_check_propagates_a_value_error_raised_during_a_run(self):
        module = load('checker_main_test', HERE / 'check-config.py')
        defect = ValueError('communicate defect')
        defect.watchdog_cleanup = {'passed': False}
        output = self.root / 'report.json'
        argv = ['check-config.py', '--baseline', sys.executable, '--output', str(output)]
        with mock.patch.object(sys, 'argv', argv), \
                mock.patch.object(module.protocol, 'run_receipts', side_effect=defect):
            with self.assertRaises(ValueError):
                module.main()
        report = json.loads(output.read_text())
        self.assertEqual(report['error'], 'interrupted: ValueError: communicate defect')
        self.assertEqual(report['watchdog_cleanup'], {'passed': False})

    def test_cancelled_outer_config_check_still_writes_a_failed_report(self):
        report = self.check_main('check-outer-config.py', ['--runner', sys.executable])
        self.assertFalse(report['passed'])
        self.assertEqual(report['error'], 'interrupted: KeyboardInterrupt: cancel')
        self.assertEqual(report['released_children'], [])


if __name__ == '__main__':
    unittest.main()
