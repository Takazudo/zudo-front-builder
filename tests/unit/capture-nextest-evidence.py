#!/usr/bin/env python3
"""Offline contract tests: the fake Cargo child never builds or runs Rust."""
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[2] / 'scripts/capture-nextest-evidence.py'
FAKE = '''#!/usr/bin/env python3
import datetime, json, os, pathlib, sys, uuid
if sys.argv[1:] == ['nextest', '--version']:
    print('cargo-nextest ' + os.environ.get('FAKE_VERSION', '0.9.140'))
elif sys.argv[1] == 'metadata':
    print(json.dumps({'workspace_root': os.getcwd(), 'target_directory': os.environ.get('CARGO_TARGET_DIR', os.getcwd() + '/target')}))
else:
    pathlib.Path('target').mkdir(exist_ok=True)
    pathlib.Path('target/child-ran').write_text(json.dumps(sys.argv[1:]))
    report = pathlib.Path(os.environ['FAKE_REPORT'])
    mode = os.environ.get('FAKE_MODE', 'valid')
    if mode != 'missing':
        report.parent.mkdir(parents=True, exist_ok=True)
        stamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='milliseconds')
        if mode == 'stale-run': stamp = '2000-01-01T00:00:00Z'
        xml = '<testsuites uuid="' + str(uuid.uuid4()) + '" timestamp="' + stamp + '"><testsuite><testcase name="ignored"><skipped type="ignored"/></testcase><testcase name="unexecuted"><skipped type="no_run"/></testcase></testsuite></testsuites>'
        if mode == 'invalid': xml = 'broken xml'
        if mode == 'replay': xml = os.environ['FAKE_REPLAY']
        report.write_text(xml)
        if mode == 'stale-file': os.utime(report, (1, 1))
    if os.environ.get('FAKE_CHANGE_CONFIG'): pathlib.Path('.config/nextest.toml').write_text('')
    if os.environ.get('FAKE_CHANGE_SOURCE'): pathlib.Path('source.rs').write_text('changed')
    print('child stdout')
    print('child stderr', file=sys.stderr)
    sys.exit(int(os.environ.get('FAKE_EXIT', '0')))
'''


class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        subprocess.run(['git', 'init', '-q', str(self.workspace)], check=True)
        subprocess.run(['git', '-C', str(self.workspace), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-q', '--allow-empty', '-m', 'fixture'], check=True)
        self.cargo = self.root / 'cargo'
        self.cargo.write_text(FAKE)
        self.cargo.chmod(0o755)
        (self.workspace / '.gitignore').write_text('target/\n')
        (self.workspace / 'source.rs').write_text('original')
        subprocess.run(['git', '-C', str(self.workspace), 'add', 'source.rs', '.gitignore'], check=True)
        subprocess.run(['git', '-C', str(self.workspace), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-q', '-m', 'source fixture'], check=True)
        self.config = self.workspace / '.config/nextest.toml'
        self.config.parent.mkdir()
        self.config.write_text('[profile.ci.junit]\npath = "junit.xml"\n')
        self.report = self.workspace / 'target/nextest/ci/junit.xml'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CARGO_', 'NEXTEST_', 'MIRI_', 'FAKE_'))}
        self.counter = 0

    def capture(self, mode='valid', code=0, options=(), forwarded=(), env=None):
        self.counter += 1
        output = self.root / f'evidence-{self.counter}'
        environment = dict(self.env, FAKE_REPORT=str(self.report), FAKE_MODE=mode, FAKE_EXIT=str(code))
        environment.update(env or {})
        result = subprocess.run([sys.executable, str(HELPER), '--cargo', str(self.cargo), '--output', str(output), '--profile', 'ci', *options, '--', *forwarded], cwd=self.workspace, env=environment, capture_output=True, text=True)
        receipt = json.loads((output / 'receipt.json').read_text())
        self.assertEqual(result.returncode, receipt['wrapper_exit'], result.stderr)
        return result.returncode, output, receipt

    def test_original_bytes_command_and_sha_bound(self):
        code, output, receipt = self.capture(forwarded=('-p', 'zfb-watcher', '-E', 'test(recursive_dirs)'))
        self.assertEqual(code, 0)
        self.assertEqual((output / 'junit.xml').read_bytes(), self.report.read_bytes())
        self.assertIn(b'type="ignored"', (output / 'junit.xml').read_bytes())
        self.assertIn(b'type="no_run"', (output / 'junit.xml').read_bytes())
        self.assertEqual(receipt['source_sha'], receipt['source_sha_after'])
        self.assertEqual(receipt['command'][-4:], ['-p', 'zfb-watcher', '-E', 'test(recursive_dirs)'])
        self.assertEqual((output / 'stdout.log').read_text(), 'child stdout\n')
        self.assertEqual((output / 'stderr.log').read_text(), 'child stderr\n')

    def test_cargo_target_does_not_relocate_nextest_store(self):
        for options, environment in [((), {'CARGO_TARGET_DIR': str(self.root / 'env-target')}), (('--target-dir', str(self.root / 'cli-target')), {})]:
            with self.subTest(options=options):
                code, _, receipt = self.capture(options=options, env=environment)
                self.assertEqual(code, 0)
                self.assertEqual(receipt['junit_destination'], str(self.report))
                self.assertNotEqual(receipt['cargo_target_directory'], str(self.workspace / 'target'))

    def test_custom_store_and_inherited_path(self):
        self.config.write_text('[store]\ndir="reports"\n[profile.parent.junit]\npath="suite.xml"\n[profile.ci]\ninherits="parent"\n')
        self.report = self.workspace / 'reports/ci/suite.xml'
        self.assertEqual(self.capture()[0], 0)

    def test_absolute_report_path_and_explicit_config(self):
        config = self.root / 'explicit.toml'
        self.report = self.root / 'absolute/report.xml'
        config.write_text('[profile.ci.junit]\npath=' + json.dumps(str(self.report)) + '\n')
        code, _, receipt = self.capture(options=('--config-file', str(config)))
        self.assertEqual(code, 0)
        self.assertEqual(receipt['config_inputs'][0]['path'], str(config))

    def test_tool_config_priority_and_workspace_override(self):
        first, second = self.root / 'first.toml', self.root / 'second.toml'
        first.write_text('[store]\ndir="first"\n[profile.ci.junit]\npath="first.xml"\n')
        second.write_text('[store]\ndir="second"\n[profile.ci.junit]\npath="second.xml"\n')
        self.report = self.workspace / 'first/ci/junit.xml'
        code, _, _ = self.capture(options=('--tool-config-file', f'first:{first}', '--tool-config-file', f'second:{second}'))
        self.assertEqual(code, 0)

    def test_sequential_commands_cannot_overwrite_archive(self):
        _, first, _ = self.capture()
        original = (first / 'junit.xml').read_bytes()
        _, second, _ = self.capture()
        self.assertEqual((first / 'junit.xml').read_bytes(), original)
        self.assertEqual((second / 'preexisting.xml').read_bytes(), original)
        self.assertNotEqual((second / 'junit.xml').read_bytes(), original)

    def test_missing_report_cannot_reuse_previous(self):
        self.capture()
        code, output, receipt = self.capture(mode='missing')
        self.assertEqual(code, 4)
        self.assertTrue((output / 'preexisting.xml').exists())
        self.assertFalse((output / 'junit.xml').exists())
        self.assertEqual(receipt['child_exit'], 0)
        self.assertEqual(receipt['evidence']['status'], 'missing')

    def test_rejected_report_retains_diagnostic_bytes(self):
        for mode in ('stale-file', 'stale-run', 'invalid'):
            with self.subTest(mode=mode):
                code, output, receipt = self.capture(mode=mode)
                self.assertEqual(code, 4)
                self.assertEqual(receipt['evidence']['status'], 'missing')
                self.assertTrue((output / 'candidate.xml').exists())
                self.assertFalse((output / 'junit.xml').exists())

    def test_replayed_original_is_rejected(self):
        self.capture()
        original = self.report.read_text()
        code, _, receipt = self.capture(mode='replay', env={'FAKE_REPLAY': original})
        self.assertEqual(code, 4)
        self.assertEqual(receipt['evidence']['status'], 'missing')

    def test_nonzero_child_preserved_with_or_without_xml(self):
        for mode in ('valid', 'missing', 'invalid'):
            with self.subTest(mode=mode):
                code, _, receipt = self.capture(mode=mode, code=100)
                self.assertEqual(code, 100)
                self.assertEqual(receipt['child_exit'], 100)
                self.assertEqual(receipt['evidence']['status'], 'captured' if mode == 'valid' else 'missing')

    def test_unsupported_inputs_do_not_start_child(self):
        for options, forwarded, env in [((), ('--no-run',), {}), ((), ('--profile=other',), {}), ((), (), {'FAKE_VERSION': '0.9.141'}), ((), (), {'MIRI_SYSROOT': '/miri'}), ((), (), {'MIRI_SYSROOT': ''})]:
            with self.subTest(forwarded=forwarded, env=env):
                code, _, receipt = self.capture(options=options, forwarded=forwarded, env=env)
                self.assertEqual(code, 2)
                self.assertIsNone(receipt['child_exit'])
                self.assertFalse((self.workspace / 'target/child-ran').exists())

    def test_disabled_junit_fails_before_execution(self):
        self.config.write_text('[profile.ci]\n')
        self.assertEqual(self.capture()[0], 2)
        self.assertFalse((self.workspace / 'target/child-ran').exists())

    def test_changed_configuration_is_missing_evidence(self):
        code, _, receipt = self.capture(env={'FAKE_CHANGE_CONFIG': '1'})
        self.assertEqual(code, 4)
        self.assertEqual(receipt['evidence']['status'], 'missing')
        self.assertIn('changed', receipt['evidence']['reason'])
        self.assertFalse((_ / 'junit.xml').exists())

    def test_tracked_destination_is_refused_without_removing_source(self):
        self.report = self.workspace / 'source.rs'
        self.config.write_text('[profile.ci.junit]\npath=' + json.dumps(str(self.report)) + '\n')
        code, _, receipt = self.capture()
        self.assertEqual(code, 2)
        self.assertIn('collides with tracked source', receipt['evidence']['reason'])
        self.assertEqual(self.report.read_text(), 'original')
        self.assertFalse((self.workspace / 'target/child-ran').exists())

    def test_source_changes_without_head_change_reject_evidence(self):
        code, output, receipt = self.capture(env={'FAKE_CHANGE_SOURCE': '1'})
        self.assertEqual(code, 4)
        self.assertEqual(receipt['source_sha'], receipt['source_sha_after'])
        self.assertNotEqual(receipt['source_fingerprint'], receipt['source_fingerprint_after'])
        self.assertTrue((output / 'candidate.xml').exists())
        self.assertFalse((output / 'junit.xml').exists())


if __name__ == '__main__':
    unittest.main()
