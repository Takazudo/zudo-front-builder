#!/usr/bin/env python3
"""Focused refusal coverage for accept.py's executable verification.

Runs with `python3 -I prototypes/direct-v8/test_accept.py` from the
repository root. No Cargo build, no second target directory: the runners are
tiny shell scripts standing in for the real executables, so the test proves
the refusal logic, not any engine behavior.
"""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("accept", HERE / "accept.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


def script(directory, name, body):
    path = Path(directory) / name
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o755)
    return path


def receipt(engine, ok=True, value=None):
    document = {"engine": engine, "ok": ok}
    if ok:
        document["value"] = accept.PROBE_VALUE if value is None else value
    else:
        document["error"] = "config evaluation error: probe failed"
    return "echo 'ZFB_HOST_RECEIPT " + json.dumps(document) + "'"


class VerifyRunnerTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def test_accepts_the_right_engine_and_value_and_binds_the_digest(self):
        path = script(self.dir.name, "candidate", receipt(accept.CANDIDATE_ENGINE))
        result = accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")
        self.assertEqual(result["engine"], accept.CANDIDATE_ENGINE)
        self.assertEqual(result["sha256"], accept.sha(path))
        self.assertEqual(result["receipt"]["value"], accept.PROBE_VALUE)

    def test_refuses_a_missing_executable(self):
        with self.assertRaisesRegex(accept.Refusal, "missing executable"):
            accept.verify_runner(Path(self.dir.name) / "absent", accept.CANDIDATE_ENGINE, "candidate")

    def test_refuses_a_non_executable_file(self):
        path = Path(self.dir.name) / "plain"
        path.write_text("not a runner")
        path.chmod(0o644)
        with self.assertRaisesRegex(accept.Refusal, "is not executable"):
            accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")

    def test_refuses_the_feature_disabled_runner(self):
        path = script(self.dir.name, "candidate",
                      "echo 'direct_v8 requires the direct-v8 feature' >&2; exit 2")
        with self.assertRaisesRegex(accept.Refusal, "feature-disabled form"):
            accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")

    def test_refuses_the_baseline_passed_as_the_candidate(self):
        path = script(self.dir.name, "candidate", receipt(accept.BASELINE_ENGINE))
        with self.assertRaisesRegex(accept.Refusal, "reports engine .*wrong executable"):
            accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate")

    def test_refuses_a_failed_probe_and_a_wrong_probe_value(self):
        failed = script(self.dir.name, "failed", receipt(accept.CANDIDATE_ENGINE, ok=False))
        with self.assertRaisesRegex(accept.Refusal, "not the expected value"):
            accept.verify_runner(failed, accept.CANDIDATE_ENGINE, "candidate")
        wrong = script(self.dir.name, "wrong", receipt(accept.CANDIDATE_ENGINE, value={"probe": 41}))
        with self.assertRaisesRegex(accept.Refusal, "not the expected value"):
            accept.verify_runner(wrong, accept.CANDIDATE_ENGINE, "candidate")
        silent = script(self.dir.name, "silent", "exit 0")
        with self.assertRaisesRegex(accept.Refusal, "0 receipt"):
            accept.verify_runner(silent, accept.CANDIDATE_ENGINE, "candidate")
        crash = script(self.dir.name, "crash", receipt(accept.CANDIDATE_ENGINE) + "; exit 101")
        with self.assertRaisesRegex(accept.Refusal, "exit 101"):
            accept.verify_runner(crash, accept.CANDIDATE_ENGINE, "candidate")

    def test_refuses_a_probe_that_never_answers(self):
        path = script(self.dir.name, "hang", "sleep 5")
        with self.assertRaisesRegex(accept.Refusal, "watchdog expired"):
            accept.verify_runner(path, accept.CANDIDATE_ENGINE, "candidate", timeout=1)


class ParsersTest(unittest.TestCase):
    def test_reads_a_pass_from_either_report_shape(self):
        self.assertTrue(accept.report_passed({"passed": True, "rows": []}))
        self.assertFalse(accept.report_passed({"passed": False}))
        self.assertFalse(accept.report_passed({"passed": "yes"}))
        self.assertTrue(accept.report_passed({"results": [{"passed": True}, {"passed": True}]}))
        self.assertFalse(accept.report_passed({"results": [{"passed": True}, {"passed": False}]}))
        self.assertFalse(accept.report_passed({"results": []}))
        self.assertFalse(accept.report_passed({"corpus_sha256": "x"}))
        self.assertFalse(accept.report_passed(None))


    def test_parses_the_cargo_test_summary(self):
        self.assertEqual(accept.parse_test_result("x\ntest result: ok. 40 passed; 0 failed; 0 ignored\n"),
                         {"status": "ok", "passed": 40, "failed": 0})
        self.assertEqual(accept.parse_test_result("test result: FAILED. 39 passed; 1 failed; 0 ignored"),
                         {"status": "FAILED", "passed": 39, "failed": 1})
        self.assertIsNone(accept.parse_test_result("no summary"))

    def test_discovers_exactly_one_lib_test_executable(self):
        lines = "\n".join([
            "warning: ignored",
            json.dumps({"reason": "compiler-artifact", "target": {"kind": ["lib"], "name": "zfb"},
                        "profile": {"test": False}, "executable": None}),
            json.dumps({"reason": "compiler-artifact", "target": {"kind": ["bin"], "name": "zfb"},
                        "profile": {"test": True}, "executable": "/t/zfb-bin"}),
            json.dumps({"reason": "compiler-artifact", "target": {"kind": ["lib"], "name": "zfb"},
                        "profile": {"test": True}, "executable": "/t/zfb-lib"}),
        ])
        self.assertEqual(accept.discover_wrapper_test_binary(lines), "/t/zfb-lib")
        with self.assertRaisesRegex(accept.Refusal, "exactly one"):
            accept.discover_wrapper_test_binary(lines + "\n" + lines.splitlines()[-1])
        with self.assertRaisesRegex(accept.Refusal, "exactly one"):
            accept.discover_wrapper_test_binary("")


class AcceptanceIntegrityTest(unittest.TestCase):
    """Exercise main's wiring with fake builds/gates, never native evidence."""

    def run_acceptance(self, *, generated=False, mutate_at=None, name="outer_config",
                       mutation="rewrite", fail_at=None):
        with tempfile.TemporaryDirectory() as directory:
            # accept.ROOT is canonical in production; macOS TMPDIR is a /var -> /private/var alias.
            root = Path(directory).resolve()
            target = root / "target"
            examples = target / "debug" / "examples"
            examples.mkdir(parents=True)
            paths = {
                "baseline": script(examples, "direct_v8_baseline", receipt(accept.BASELINE_ENGINE)),
                "candidate": script(examples, "direct_v8", receipt(accept.CANDIDATE_ENGINE)),
                "outer_config": script(examples, "direct_v8_outer_config", "exit 0"),
                "postprocessor": script(examples, "direct_v8_postprocess", "exit 0"),
                "zfb": script(target / "debug", "zfb", "exit 0"),
                "wrapper_test_binary": script(target / "debug", "wrapper-test", "exit 0"),
            }
            original = {key: accept.sha(path) for key, path in paths.items()}
            visited = []

            def visit(label):
                visited.append(label)
                if label != mutate_at:
                    return
                path = paths[name]
                if mutation == "delete":
                    path.unlink()
                elif mutation == "chmod":
                    path.chmod(0o644)
                else:
                    path.write_text(path.read_text() + "# replaced after pinning\n")

            def run(instance, label, argv, **kwargs):
                visit(label)
                instance.steps.append({"label": label, "exit_code": 0})
                stdout = ""
                if label in ("services-tests", "candidate-lib-tests"):
                    stdout = "test result: ok. 1 passed; 0 failed; 0 ignored;"
                elif label == "build-wrapper-test":
                    stdout = json.dumps({
                        "reason": "compiler-artifact", "target": {"kind": ["lib"], "name": "zfb"},
                        "profile": {"test": True}, "executable": str(paths["wrapper_test_binary"]),
                    })
                return SimpleNamespace(args=argv, stdout=stdout)

            def gate(instance, label, argv, output, counts, report_path=None):
                visit(label)
                return {"passed": label != fail_at, "output": str(output), "counts": {}}

            out = target / "acceptance-test"
            argv = ["accept.py", "--output", str(out)]
            if generated:
                # These paths are not executed: only engine-probe shell stand-ins run.
                argv += ["--generated", "--esbuild", "/unused/esbuild", "--chromium", "/unused/chromium",
                         "--node", "/unused/node", "--rustc", "/unused/rustc"]
            with mock.patch.object(accept, "ROOT", root), \
                    mock.patch.object(accept.Acceptance, "environment", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "prepare_icu", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "cargo_target_dir", return_value=target), \
                    mock.patch.object(accept.Acceptance, "run", new=run), \
                    mock.patch.object(accept.Acceptance, "gate", new=gate), \
                    mock.patch.object(accept.sys, "argv", argv), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                status = accept.main()
            return status, json.loads((out / "acceptance.json").read_text()), original, visited

    def test_unchanged_executables_pass_in_both_modes(self):
        for generated in (False, True):
            with self.subTest(generated=generated):
                status, report, original, _ = self.run_acceptance(generated=generated)
                names = set(original) if generated else {"baseline", "candidate", "outer_config"}
                self.assertEqual(status, 0)
                self.assertTrue(report["passed"])
                self.assertEqual(report["executables_after"], {name: original[name] for name in names})
                checks = report["executable_checks"]
                self.assertTrue(all(check["passed"] for check in checks))
                self.assertEqual(checks[-1]["after"], "final verification")
                self.assertEqual(checks[-1]["sha256"], report["executables_after"])

    def test_outer_config_gate_checks_all_three_executables_without_generated(self):
        for name in ("baseline", "candidate", "outer_config"):
            with self.subTest(name=name):
                status, report, _, _ = self.run_acceptance(mutate_at="gate-outer-config", name=name)
                self.assertEqual(status, 1)
                self.assertFalse(report["passed"])
                self.assertIn(name, report["error"])
                self.assertIn("bytes changed", report["error"])

    def test_every_recorded_executable_is_checked_after_generated_gate(self):
        for name in ("baseline", "candidate", "outer_config", "postprocessor", "zfb", "wrapper_test_binary"):
            with self.subTest(name=name):
                status, report, _, _ = self.run_acceptance(
                    generated=True, mutate_at="gate-generated", name=name)
                self.assertEqual(status, 1)
                self.assertFalse(report["passed"])
                self.assertIn(name, report["error"])

    def test_fixture_mutation_is_refused_before_generated_gate(self):
        for name in ("postprocessor", "zfb", "wrapper_test_binary"):
            with self.subTest(name=name):
                status, report, _, visited = self.run_acceptance(
                    generated=True, mutate_at="prepare-fixtures", name=name)
                self.assertEqual(status, 1)
                self.assertIn(name, report["error"])
                self.assertNotIn("gate-generated", visited)

    def test_builds_cannot_repin_a_previously_built_executable(self):
        for label, name in (("build-zfb", "postprocessor"), ("build-wrapper-test", "zfb")):
            with self.subTest(label=label):
                status, report, _, visited = self.run_acceptance(
                    generated=True, mutate_at=label, name=name)
                self.assertEqual(status, 1)
                self.assertIn(name, report["error"])
                self.assertNotIn("prepare-fixtures", visited)

    def test_missing_or_nonexecutable_files_write_failure_receipts(self):
        for mutation in ("delete", "chmod"):
            for generated, label, name in ((False, "gate-outer-config", "outer_config"),
                                            (True, "gate-generated", "postprocessor")):
                with self.subTest(mutation=mutation, name=name):
                    status, report, _, _ = self.run_acceptance(
                        generated=generated, mutate_at=label, name=name, mutation=mutation)
                    self.assertEqual(status, 1)
                    self.assertFalse(report["passed"])
                    self.assertIn(name, report["error"])

    def test_missing_generated_build_output_is_refused_at_registration(self):
        for mutation in ("delete", "chmod"):
            with self.subTest(mutation=mutation):
                status, report, _, visited = self.run_acceptance(
                    generated=True, mutate_at="build-postprocessor", name="postprocessor", mutation=mutation)
                self.assertEqual(status, 1)
                self.assertIn("postprocessor", report["error"])
                self.assertNotIn("build-zfb", visited)

    def test_read_error_during_hashing_is_a_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            path = script(directory, "helper", "exit 0")
            with mock.patch.object(accept, "sha", side_effect=OSError("file disappeared during read")):
                with self.assertRaisesRegex(accept.Refusal, "helper: cannot read executable"):
                    accept.record_executable(path, "helper")

    def test_completed_gates_survive_a_later_integrity_refusal(self):
        status, report, _, _ = self.run_acceptance(mutate_at="gate-services", name="candidate")
        self.assertEqual(status, 1)
        self.assertTrue(report["gates"]["config"]["passed"])
        self.assertNotIn("http", report["gates"])
        check = report["executable_checks"][-1]
        self.assertFalse(check["passed"])
        self.assertEqual(check["after"], "services")
        self.assertIn("candidate", check["error"])

    def test_gate_failure_is_not_overwritten_by_successful_integrity_checks(self):
        status, report, _, _ = self.run_acceptance(generated=True, fail_at="gate-config")
        self.assertEqual(status, 1)
        self.assertFalse(report["passed"])
        self.assertEqual(report["error"], "gates failed: config")


if __name__ == "__main__":
    unittest.main()
