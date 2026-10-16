#!/usr/bin/env python3
"""Failure-evidence regressions; these do not exercise Cargo or either engine.

Run all focused suites from the repository root with:
  python3 -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_accept*.py'

Tiny Python/shell subprocesses exercise actual launch, exit, watchdog and gate
report handling. Mocked setup failures test the top-level receipt boundary.
"""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("accept_failures_subject", HERE / "accept.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


class CommandFailureTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.acceptance = accept.Acceptance(SimpleNamespace(output=self.root, guard=None))
        self.acceptance.logs.mkdir()

    def run_python(self, source, **kwargs):
        return self.acceptance.run("command", [sys.executable, "-I", "-c", source],
                                   cwd=self.root, **kwargs)

    def assert_failed_step(self, exit_code):
        self.assertEqual(len(self.acceptance.steps), 1)
        step = self.acceptance.steps[0]
        self.assertIs(step["passed"], False)
        self.assertEqual(step["exit_code"], exit_code)
        self.assertGreaterEqual(step["seconds"], 0)
        self.assertIn("error", step)
        return step

    def test_missing_command_is_a_recorded_refusal_not_an_exit_code(self):
        with self.assertRaisesRegex(accept.Refusal, "cannot execute"):
            self.acceptance.run("command", [self.root / "missing-tool"], cwd=self.root)
        step = self.assert_failed_step(None)
        self.assertIn("FileNotFoundError", step["error"])
        self.assertEqual((self.acceptance.logs / "command.stdout").read_bytes(), b"")
        self.assertEqual((self.acceptance.logs / "command.stderr").read_bytes(), b"")

    def test_missing_working_directory_is_also_a_launch_refusal(self):
        with self.assertRaisesRegex(accept.Refusal, "cannot execute"):
            self.acceptance.run("command", [sys.executable, "-V"], cwd=self.root / "absent")
        self.assert_failed_step(None)

    def test_nonzero_exit_keeps_both_streams_and_the_actual_exit_code(self):
        with self.assertRaisesRegex(accept.Refusal, "exit 7"):
            self.run_python("import sys; print('before failure'); print('why', file=sys.stderr); sys.exit(7)")
        self.assert_failed_step(7)
        self.assertEqual((self.acceptance.logs / "command.stdout").read_text(), "before failure\n")
        self.assertEqual((self.acceptance.logs / "command.stderr").read_text(), "why\n")

    def test_success_is_marked_passed_only_after_output_is_saved(self):
        result = self.run_python("print('complete')")
        self.assertEqual(result.returncode, 0)
        self.assertIs(self.acceptance.steps[0]["passed"], True)
        self.assertEqual(self.acceptance.steps[0]["exit_code"], 0)
        self.assertNotIn("error", self.acceptance.steps[0])
        self.assertEqual((self.acceptance.logs / "command.stdout").read_text(), "complete\n")

    def test_real_watchdog_keeps_output_without_claiming_clean_completion(self):
        with self.assertRaisesRegex(accept.Refusal, "watchdog expired"):
            self.run_python("import os, time; os.write(1, b'before timeout\\n'); "
                            "os.write(2, b'still running\\n'); time.sleep(30)", timeout=1)
        step = self.assert_failed_step(None)
        self.assertEqual(step["watchdog_seconds"], 1)
        self.assertEqual((self.acceptance.logs / "command.stdout").read_bytes(), b"before timeout\n")
        self.assertEqual((self.acceptance.logs / "command.stderr").read_bytes(), b"still running\n")

    def test_watchdog_preserves_partial_utf8_bytes_and_guard_context(self):
        self.acceptance.args.guard = self.root / "heavy-guard.sh"
        timeout = subprocess.TimeoutExpired("fixture", 2, output=b"partial\xf0\x9f", stderr=b"bad\xff")
        with mock.patch.object(accept.subprocess, "run", side_effect=timeout):
            with self.assertRaises(accept.Refusal):
                self.run_python("unused", guarded=True, timeout=2, env={"CARGO_BUILD_JOBS": "3"})
        step = self.assert_failed_step(None)
        self.assertEqual(step["argv"][:3], ["bash", str(self.acceptance.args.guard), "--"])
        self.assertTrue(step["guarded"])
        self.assertEqual(step["env"]["CARGO_BUILD_JOBS"], "3")
        self.assertEqual((self.acceptance.logs / "command.stdout").read_bytes(), b"partial\xf0\x9f")
        self.assertEqual((self.acceptance.logs / "command.stderr").read_bytes(), b"bad\xff")

    def test_silent_watchdog_still_creates_empty_log_files(self):
        with mock.patch.object(accept.subprocess, "run", side_effect=subprocess.TimeoutExpired("fixture", 2)):
            with self.assertRaises(accept.Refusal):
                self.run_python("unused", timeout=2)
        self.assert_failed_step(None)
        self.assertEqual((self.acceptance.logs / "command.stdout").read_bytes(), b"")
        self.assertEqual((self.acceptance.logs / "command.stderr").read_bytes(), b"")


class GateFailureTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.acceptance = accept.Acceptance(SimpleNamespace(output=self.root, guard=None))
        self.acceptance.logs.mkdir()
        self.checker = self.root / "checker.py"
        self.output = self.root / "report.json"

    def gate(self, payload, *, code=0, counts=accept.rows_counts, report_path=None):
        destination = Path(report_path or self.output)
        source = "from pathlib import Path\nimport sys\n"
        if payload is not None:
            source += f"Path({str(destination)!r}).write_bytes({payload!r})\n"
        source += f"sys.exit({code})\n"
        self.checker.write_text(source)
        return self.acceptance.gate("fixture-gate", [str(self.checker)], self.output,
                                    counts, report_path=report_path)

    def test_missing_report_is_a_failed_gate_with_a_diagnostic(self):
        result = self.gate(None)
        self.assertIs(result["passed"], False)
        self.assertIsNone(result["counts"])
        self.assertIn("FileNotFoundError", " ".join(result["errors"]))

    def test_invalid_json_encoding_and_nonobject_reports_are_failed_gates(self):
        for payload in (b'{"passed":', b"\xff", b"null", b"[]", b"42"):
            with self.subTest(payload=payload):
                result = self.gate(payload)
                self.assertIs(result["passed"], False)
                self.assertIsNone(result["counts"])
                self.assertIn("invalid gate report", " ".join(result["errors"]))
                self.assertEqual(self.output.read_bytes(), payload)

    def test_malformed_count_structures_cannot_crash_or_pass(self):
        fixtures = [
            ({"passed": True, "rows": None}, accept.rows_counts),
            ({"passed": True, "rows": [None]}, accept.rows_counts),
            ({"results": True}, accept.engine_counts),
            ({"results": [None]}, accept.engine_counts),
            ({"passed": True, "baseline": None}, accept.generated_counts),
            ({"passed": True, "browser": []}, accept.generated_counts),
        ]
        for report, counts in fixtures:
            with self.subTest(report=report):
                result = self.gate(json.dumps(report).encode(), counts=counts)
                self.assertIs(result["passed"], False)
                self.assertIsNone(result["counts"])
                self.assertTrue(result["errors"])

    def test_failed_command_cannot_be_overridden_by_a_passing_report(self):
        result = self.gate(b'{"passed": true, "rows": [{"name": "fixture", "passed": true}]}', code=7)
        self.assertIs(result["passed"], False)
        self.assertEqual(result["counts"]["rows_passed"], 1)
        self.assertIn("exit 7", " ".join(result["errors"]))

    def test_failed_command_and_invalid_report_keep_both_diagnostics(self):
        result = self.gate(b'{"passed":', code=7)
        self.assertIs(result["passed"], False)
        self.assertEqual(len(result["errors"]), 2)
        self.assertIn("exit 7", result["errors"][0])
        self.assertIn("invalid gate report", result["errors"][1])

    def test_valid_failure_report_retains_counts(self):
        result = self.gate(b'{"passed": false, "rows": [{"passed": false}]}')
        self.assertIs(result["passed"], False)
        self.assertEqual(result["counts"]["rows"], 1)
        self.assertEqual(result["counts"]["rows_passed"], 0)
        self.assertTrue(result["errors"])

    def test_valid_success_preserves_all_existing_count_formats(self):
        # These are synthetic successes, not native receipts. Empty evidence
        # and one-sided engine results are now deliberately rejected.
        row = {"name": "fixture", "passed": True}
        mapped_throw = {"name": "authored-mapped-throw", "passed": True}
        fixtures = [
            ({"passed": True, "rows": [row]}, accept.rows_counts),
            ({"results": [{"engine": engine, "passed": True, "rows": [row],
                           "expected_count": 1, "receipt_count": 1, "exit_code": 0}
                          for engine in (accept.BASELINE_ENGINE, accept.CANDIDATE_ENGINE)]}, accept.engine_counts),
            ({"passed": True, "snapshot_manifest_sha256": "a" * 64,
              "baseline": {"engine": accept.BASELINE_ENGINE, "passed": True, "rows": [row, mapped_throw]},
              "candidate": {"engine": accept.CANDIDATE_ENGINE, "passed": True, "rows": [row, mapped_throw]},
              "byte_and_value_parity": [row], "browser": {"passed": True, "exit_code": 0}}, accept.generated_counts),
        ]
        for report, counts in fixtures:
            with self.subTest(counts=counts.__name__):
                result = self.gate(json.dumps(report).encode(), counts=counts)
                self.assertIs(result["passed"], True)
                self.assertEqual(result["counts"], counts(report))
                self.assertNotIn("errors", result)

    def test_explicit_report_path_is_used_for_generated_style_gate(self):
        output = self.root / "generated-results.json"
        result = self.gate(b'{"passed": true, "rows": [{"name": "fixture", "passed": true}]}', report_path=output)
        self.assertIs(result["passed"], True)
        self.assertEqual(result["output"], str(output))
        self.assertFalse(self.output.exists())

    def test_unexpected_checker_summary_bugs_are_not_swallowed(self):
        with self.assertRaisesRegex(RuntimeError, "summary bug"):
            self.gate(b'{"passed": true}', counts=mock.Mock(side_effect=RuntimeError("summary bug")))

    def test_report_passed_rejects_nonlist_engine_results_without_throwing(self):
        for results in (None, True, 3, "yes", {"passed": True}, []):
            with self.subTest(results=results):
                self.assertIs(accept.report_passed({"results": results}), False)


class ProbeFailureTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def test_malformed_and_nonobject_receipts_are_explicit_refusals(self):
        for payload in ('{"engine":', 'null', '[]', '42', 'true'):
            with self.subTest(payload=payload):
                path = self.root / "runner"
                path.write_text("#!/bin/sh\nprintf '%s\\n' 'ZFB_HOST_RECEIPT " + payload + "'\n")
                path.chmod(0o755)
                with self.assertRaisesRegex(accept.Refusal, "candidate:.*receipt"):
                    accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")

    def test_existing_executable_with_missing_interpreter_is_a_refusal(self):
        path = self.root / "runner"
        path.write_text(f"#!{self.root}/missing-interpreter\n")
        path.chmod(0o755)
        with self.assertRaisesRegex(accept.Refusal, "candidate:.*cannot execute"):
            accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")


class MainFailureReceiptTest(unittest.TestCase):
    def run_failure(self, error, *, propagates=False):
        with tempfile.TemporaryDirectory() as directory:
            # accept.ROOT is canonical in production; macOS TMPDIR is a /var -> /private/var alias.
            root = Path(directory).resolve()
            out = root / "target" / "failure"

            def metadata(instance):
                # A prior observation must remain in the final failed receipt.
                instance.steps.append({"label": "prior-observation", "exit_code": 0, "passed": True})
                raise error

            with mock.patch.object(accept, "ROOT", root), \
                    mock.patch.object(accept.Acceptance, "environment", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "prepare_icu", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "cargo_target_dir", new=metadata), \
                    mock.patch.object(accept.sys, "argv", ["accept.py", "--output", str(out)]), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                if propagates:
                    with self.assertRaises(type(error)) as raised:
                        accept.main()
                    self.assertIs(raised.exception, error)
                else:
                    self.assertEqual(accept.main(), 1)
            report = json.loads((out / "acceptance.json").read_text())
            self.assertIs(report["passed"], False)
            self.assertEqual(report["steps"], [{"label": "prior-observation", "exit_code": 0, "passed": True}])
            self.assertEqual(report["gates"], {})
            self.assertIn(str(error), report["error"])
            return report

    def test_expected_operational_errors_write_failure_receipts(self):
        for error in (FileNotFoundError("tool missing"), PermissionError("cannot read input"),
                      ValueError("invalid metadata"), subprocess.CalledProcessError(7, "rustup")):
            with self.subTest(error=type(error).__name__):
                report = self.run_failure(error)
                self.assertIn(type(error).__name__, report["error"])

    def test_existing_refusals_still_write_failure_receipts(self):
        self.run_failure(accept.Refusal("candidate refused"))

    def test_unexpected_bugs_write_failure_receipts_and_still_propagate(self):
        report = self.run_failure(RuntimeError("unexpected harness bug"), propagates=True)
        self.assertIn("RuntimeError", report["error"])

    def test_interrupt_writes_failure_receipt_and_still_propagates(self):
        report = self.run_failure(KeyboardInterrupt(), propagates=True)
        self.assertIn("KeyboardInterrupt", report["error"])


class OutputBoundaryTest(unittest.TestCase):
    """The target/ boundary holds for symlinked spellings without trusting them (macOS TMPDIR shape)."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        base = Path(directory.name).resolve()
        self.root = base / "repo"
        (self.root / "target").mkdir(parents=True)
        self.outside = base / "outside"
        self.outside.mkdir()
        self.alias = base / "alias"
        self.alias.symlink_to(self.root, target_is_directory=True)

    def main(self, output):
        def refuse(instance):
            raise accept.Refusal("stop after the boundary check")

        with mock.patch.object(accept, "ROOT", self.root), \
                mock.patch.object(accept.Acceptance, "environment", return_value={"fixture": True}), \
                mock.patch.object(accept.Acceptance, "prepare_icu", return_value={"fixture": True}), \
                mock.patch.object(accept.Acceptance, "cargo_target_dir", new=refuse), \
                mock.patch.object(accept.sys, "argv", ["accept.py", "--output", str(output)]), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return accept.main()

    def assert_refused(self, output, escaped):
        with self.assertRaises(SystemExit) as raised:
            self.main(output)
        self.assertEqual(raised.exception.code, 2)
        self.assertFalse(escaped.exists())

    def test_output_spelled_through_a_root_alias_is_accepted_at_its_canonical_path(self):
        self.assertEqual(self.main(self.alias / "target" / "run"), 1)
        report = json.loads((self.root / "target" / "run" / "acceptance.json").read_text())
        self.assertIn("stop after the boundary check", report["error"])

    def test_symlinked_target_directory_cannot_move_the_boundary(self):
        (self.root / "target").rmdir()
        (self.root / "target").symlink_to(self.outside, target_is_directory=True)
        self.assert_refused(self.root / "target" / "run", self.outside / "run")

    def test_symlink_inside_target_cannot_escape(self):
        (self.root / "target" / "link").symlink_to(self.outside, target_is_directory=True)
        self.assert_refused(self.root / "target" / "link" / "run", self.outside / "run")
        self.assert_refused(self.alias / "target" / "link" / "run", self.outside / "run")

    def test_parent_traversal_cannot_escape(self):
        self.assert_refused(self.root / "target" / ".." / ".." / "outside" / "run", self.outside / "run")


if __name__ == "__main__":
    unittest.main()
