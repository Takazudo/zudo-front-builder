import importlib.util
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import subprocess
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("runner", Path(__file__).resolve().parents[2] / "scripts/run-rosetta-nextest.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Checks(unittest.TestCase):
    def test_metadata_requires_exact_host_and_lifecycle(self):
        cargo = {"target_directory": "/target", "packages": [{"id": "worker", "name": "zfb-compiler-worker"}]}
        data = {"rust-build-meta": {"target-directory": "/target", "build-directory": "/target", "non-test-binaries": {"worker": [{"name": "compiler-worker-test-host", "kind": "bin", "path": "debug/compiler-worker-test-host"}]}}, "rust-binaries": {"life": {"binary-name": "lifecycle", "package-id": "worker", "binary-path": "/target/debug/lifecycle"}}}
        self.assertEqual(m.host_from_metadata(data, cargo), Path('/target/debug/compiler-worker-test-host'))
        data['rust-build-meta']['build-directory'] = '/shared'
        with self.assertRaises(m.Refusal):
            m.host_from_metadata(data, cargo)
        data['rust-build-meta']['build-directory'] = '/target'
        data['rust-binaries']['life']['binary-path'] = '/shared/lifecycle'
        with self.assertRaises(m.Refusal):
            m.host_from_metadata(data, cargo)
        data['rust-binaries']['life']['binary-path'] = '/target/debug/lifecycle'
        data['rust-build-meta']['non-test-binaries']['worker'][0]['path'] = '../escape'
        with self.assertRaises(m.Refusal):
            m.host_from_metadata(data, cargo)
        data['rust-build-meta']['non-test-binaries']['worker'] = []
        with self.assertRaises(m.Refusal):
            m.host_from_metadata(data, cargo)

    def test_warm_rejects_wrong_result_and_replacement(self):
        good = subprocess.CompletedProcess([], 64, b'', b'compiler-worker-test-host: started without the private worker argument\n')
        with patch.object(m, 'identity', return_value={'inode': 1}), patch.object(m.subprocess, 'run', return_value=good) as run:
            m.warm(Path('/host'), {'inode': 1})
            self.assertEqual(run.call_args.args[0], ['/host'])
            self.assertEqual(run.call_args.kwargs['timeout'], 30)
        with patch.object(m, 'identity', side_effect=[{'inode': 1}, {'inode': 2}]), patch.object(m.subprocess, 'run', return_value=good):
            with self.assertRaises(m.Refusal):
                m.warm(Path('/host'), {'inode': 1})
        good.returncode = 0
        with patch.object(m, 'identity', return_value={'inode': 1}), patch.object(m.subprocess, 'run', return_value=good):
            with self.assertRaises(m.Refusal):
                m.warm(Path('/host'), {'inode': 1})
        with patch.object(m, 'identity', return_value={'inode': 1}), patch.object(m.subprocess, 'run', side_effect=subprocess.TimeoutExpired(['/host'], 30)):
            with self.assertRaises(subprocess.TimeoutExpired):
                m.warm(Path('/host'), {'inode': 1})

    def test_failed_warm_retains_raw_evidence(self):
        evidence = {}
        bad = subprocess.CompletedProcess([], 7, b'first output\xff', b'first error')
        with patch.object(m, 'identity', return_value={'inode': 1}), patch.object(m.subprocess, 'run', return_value=bad):
            with self.assertRaises(m.Refusal):
                m.warm(Path('/host'), {'inode': 1}, evidence=evidence)
        self.assertEqual(evidence['exit'], 7)
        self.assertEqual(m.base64.b64decode(evidence['stdout_base64']), bad.stdout)
        self.assertGreaterEqual(evidence['ended_ns'], evidence['started_ns'])
        evidence = {}
        timeout = subprocess.TimeoutExpired(['/host'], 30, output=b'partial', stderr=b'error')
        with patch.object(m, 'identity', return_value={'inode': 1}), patch.object(m.subprocess, 'run', side_effect=timeout):
            with self.assertRaises(subprocess.TimeoutExpired):
                m.warm(Path('/host'), {'inode': 1}, evidence=evidence)
        self.assertEqual(evidence['execution'], 'timeout')
        self.assertEqual(evidence['stdout'], 'partial')
        self.assertEqual(evidence['stderr'], 'error')
        self.assertGreaterEqual(evidence['ended_ns'], evidence['started_ns'])

    def test_junit_missing_stale_counts_and_failure(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / 'junit.xml'
            with self.assertRaises(m.Refusal):
                m.junit_counts(f, 1, 0)
            f.write_text('<testsuites><testsuite><testcase><failure/></testcase></testsuite></testsuites>')
            self.assertEqual(m.junit_counts(f, 1, 0)['failures'], 1)
            with self.assertRaises(m.Refusal):
                m.junit_counts(f, 2, 0)
            with self.assertRaises(m.Refusal):
                m.junit_counts(f, 1, time.time_ns() + 1000000000)

    def test_all_skipped_or_wrong_test_is_not_pass(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / 'junit.xml'
            f.write_text('<testsuites><testsuite><testcase name="wanted"><skipped/></testcase></testsuite></testsuites>')
            with self.assertRaises(m.Refusal):
                m.junit_counts(f, 1, 0)
            f.write_text('<testsuites><testsuite><testcase name="other"/></testsuite></testsuites>')
            with self.assertRaises(m.Refusal):
                m.junit_counts(f, 1, 0, 'wanted')

    def test_wrong_architecture(self):
        with patch.object(m, 'output', return_value='Mach-O 64-bit executable arm64'):
            with self.assertRaises(m.Refusal):
                m.x64(Path('/host'))


    def test_native_refusal_retains_receipt_and_existing_output_untouched(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / 'evidence'
            argv = ['runner', '--source-sha', 'a' * 40, '--output', str(out), '--expected-tests', '1', '--expected-test-name', 'the_test', '--qualification-root', d, '--filter', 'all()']
            with patch.object(sys, 'argv', argv), patch.object(m, 'output', return_value='/source'), patch.object(m.platform, 'system', return_value='Linux'), patch.dict(m.os.environ, {'HEAVY_GUARD_HELD': '1', 'CARGO_TARGET_DIR': str(Path(d).resolve() / 'target')}):
                self.assertEqual(m.main(), 2)
                self.assertIn('Darwin required', (out / 'receipt.json').read_text())
                before = (out / 'receipt.json').read_bytes()
                self.assertEqual(m.main(), 2)
                self.assertEqual((out / 'receipt.json').read_bytes(), before)

    def test_missing_binary_and_corrupt_metadata_refused(self):
        with self.assertRaises(m.Refusal):
            m.identity(Path('/does-not-exist-rosetta-test'))
        with self.assertRaises(m.Refusal):
            m.host_from_metadata({'unexpected': {}}, {})

unittest.main()
