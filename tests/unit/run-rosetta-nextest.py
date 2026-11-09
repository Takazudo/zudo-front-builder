import importlib.util
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import subprocess

spec = importlib.util.spec_from_file_location("runner", Path(__file__).resolve().parents[2] / "scripts/run-rosetta-nextest.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Checks(unittest.TestCase):
    def test_metadata_requires_exact_host_and_lifecycle(self):
        cargo = {"target_directory": "/target", "packages": [{"id": "worker", "name": "zfb-compiler-worker"}]}
        data = {"rust-build-meta": {"target-directory": "/target", "non-test-binaries": {"worker": [{"name": "compiler-worker-test-host", "kind": "bin", "path": "debug/compiler-worker-test-host"}]}}, "rust-binaries": {"life": {"binary-name": "lifecycle", "package-id": "worker"}}}
        self.assertEqual(m.host_from_metadata(data, cargo), Path('/target/debug/compiler-worker-test-host'))
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

    def test_wrong_architecture(self):
        with patch.object(m, 'output', return_value='Mach-O 64-bit executable arm64'):
            with self.assertRaises(m.Refusal):
                m.x64(Path('/host'))


unittest.main()
