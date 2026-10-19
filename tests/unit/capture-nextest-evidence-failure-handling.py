#!/usr/bin/env python3
"""Offline destination-refusal and receipt-failure contract tests; no Rust runs.

Receipt write failures are injected by the fake Cargo turning the receipt path
into a directory, which fails for root too and does not depend on chmod.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[2] / 'scripts/capture-nextest-evidence.py'
FAKE = '''#!/usr/bin/env python3
import datetime, json, os, pathlib, signal, sys, uuid
def block_receipt(stage):
    if os.environ.get('FAKE_BLOCK_RECEIPT') == stage:
        (pathlib.Path(os.environ['FAKE_OUTPUT']) / 'receipt.json').mkdir(parents=True)
if sys.argv[1:] == ['nextest', '--version']:
    block_receipt('version')
    print('cargo-nextest ' + os.environ.get('FAKE_VERSION', '0.9.140'))
elif sys.argv[1] == 'metadata':
    print(json.dumps({'workspace_root': os.getcwd(), 'target_directory': os.getcwd() + '/target'}))
else:
    pathlib.Path(os.environ['FAKE_RAN']).write_text(json.dumps(sys.argv[1:]))
    report = pathlib.Path(os.environ['FAKE_REPORT'])
    report.parent.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='milliseconds')
    report.write_text('<testsuites uuid="' + str(uuid.uuid4()) + '" timestamp="' + stamp + '"><testsuite/></testsuites>')
    block_receipt('child')
    print('child stdout')
    if os.environ.get('FAKE_SIGNAL'):
        os.kill(os.getpid(), int(os.environ['FAKE_SIGNAL']))
    sys.exit(int(os.environ.get('FAKE_EXIT', '0')))
'''


class FailureHandlingTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        git = ['git', '-C', str(self.workspace), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid']
        subprocess.run(['git', 'init', '-q', str(self.workspace)], check=True)
        (self.workspace / '.gitignore').write_text('target/\n')
        (self.workspace / 'source.rs').write_text('original')
        self.config = self.workspace / '.config/nextest.toml'
        self.config.parent.mkdir()
        self.config.write_text('[profile.ci.junit]\npath = "junit.xml"\n')
        subprocess.run([*git, 'add', '.'], check=True)
        subprocess.run([*git, 'commit', '-q', '-m', 'fixture'], check=True)
        self.cargo = self.root / 'cargo'
        self.cargo.write_text(FAKE)
        self.cargo.chmod(0o755)
        self.report = self.workspace / 'target/nextest/ci/junit.xml'
        self.ran = self.root / 'child-ran'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CARGO_', 'NEXTEST_', 'MIRI_', 'FAKE_'))}

    def capture(self, output, options=(), env=None):
        environment = dict(self.env, FAKE_REPORT=str(self.report), FAKE_RAN=str(self.ran), FAKE_OUTPUT=str(output))
        environment.update(env or {})
        return subprocess.run([sys.executable, str(HELPER), '--cargo', str(self.cargo), '--output', str(output), '--profile', 'ci', *options, '--'],
                              cwd=self.workspace, env=environment, capture_output=True, text=True)

    def explicit_report(self, report):
        self.report = report
        config = self.root / 'explicit.toml'
        config.write_text('[profile.ci.junit]\npath=' + json.dumps(str(report)) + '\n')
        return ('--config-file', str(config))

    def assert_refused(self, result, created):
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse(self.ran.exists(), 'child must not start')
        self.assertIn('no receipt written', result.stderr)
        for path in created:
            self.assertFalse(os.path.lexists(path), f'{path} was created')

    def test_destination_in_workspace_is_not_created(self):
        link = self.root / 'workspace-link'
        link.symlink_to(self.workspace)
        for output, created in [(self.workspace / 'new/evidence', [self.workspace / 'new']),
                                (link / 'via-symlink', [self.workspace / 'via-symlink'])]:
            with self.subTest(output=output):
                self.assert_refused(self.capture(output), created)

    def test_dangling_symlink_destination_is_not_followed(self):
        link, target = self.root / 'dangling', self.root / 'elsewhere'
        link.symlink_to(target)
        self.assert_refused(self.capture(link), [target])
        self.assertTrue(link.is_symlink())

    def test_report_collision_leaves_destination_unchanged(self):
        report = self.root / 'reports/ci/junit.xml'
        options = self.explicit_report(report)
        for output, created in [(report, [report]), (self.root / 'reports', [self.root / 'reports']), (report / 'inside', [report])]:
            with self.subTest(output=output):
                self.assert_refused(self.capture(output, options), created)
        report.parent.mkdir(parents=True)
        report.write_bytes(b'prior report')
        self.assert_refused(self.capture(report, options), [])
        self.assertEqual(report.read_bytes(), b'prior report')

    def test_existing_destination_is_unchanged(self):
        output = self.root / 'existing'
        output.mkdir()
        (output / 'keep.txt').write_text('kept')
        self.assert_refused(self.capture(output), [])
        self.assertEqual(sorted(p.name for p in output.iterdir()), ['keep.txt'])
        self.assertEqual((output / 'keep.txt').read_text(), 'kept')

    def assert_no_receipt(self, result, output):
        self.assertIn('receipt not written', result.stderr)
        self.assertNotIn('receipt=', result.stderr)
        self.assertTrue((output / 'receipt.json').is_dir())
        self.assertEqual([p.name for p in output.iterdir() if p.name.startswith('.receipt')], [])

    def test_preflight_receipt_failure_exits_2(self):
        output = self.root / 'evidence'
        result = self.capture(output, env={'FAKE_VERSION': '0.9.141', 'FAKE_BLOCK_RECEIPT': 'version'})
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse(self.ran.exists())
        self.assertIn('receipt not written', result.stderr)

    def test_child_success_receipt_failure_exits_4_and_keeps_evidence(self):
        output = self.root / 'evidence'
        result = self.capture(output, env={'FAKE_BLOCK_RECEIPT': 'child'})
        self.assertEqual(result.returncode, 4, result.stderr)
        self.assert_no_receipt(result, output)
        self.assertEqual((output / 'junit.xml').read_bytes(), self.report.read_bytes())
        self.assertEqual((output / 'stdout.log').read_text(), 'child stdout\n')

    def test_child_failure_receipt_failure_keeps_child_status(self):
        for env, expected in [({'FAKE_EXIT': '100'}, 100), ({'FAKE_SIGNAL': str(int(signal.SIGTERM))}, 128 + signal.SIGTERM)]:
            with self.subTest(expected=expected):
                output = self.root / f'evidence-{expected}'
                result = self.capture(output, env=dict(env, FAKE_BLOCK_RECEIPT='child'))
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assert_no_receipt(result, output)
                self.assertEqual((output / 'junit.xml').read_bytes(), self.report.read_bytes())

    def test_outside_destination_still_captures(self):
        output = self.root / 'new-parent/evidence'
        result = self.capture(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads((output / 'receipt.json').read_text())
        self.assertEqual((receipt['wrapper_exit'], receipt['child_exit'], receipt['evidence']['status']), (0, 0, 'captured'))
        self.assertEqual((output / 'junit.xml').read_bytes(), self.report.read_bytes())
        self.assertIn(f'receipt={output / "receipt.json"}', result.stderr)

    def test_preflight_failure_still_records_receipt(self):
        output = self.root / 'evidence'
        result = self.capture(output, env={'FAKE_VERSION': '0.9.141'})
        self.assertEqual(result.returncode, 2, result.stderr)
        receipt = json.loads((output / 'receipt.json').read_text())
        self.assertEqual((receipt['wrapper_exit'], receipt['child_exit'], receipt['execution']), (2, None, 'not_started'))
        self.assertFalse(self.ran.exists())


if __name__ == '__main__':
    unittest.main()
