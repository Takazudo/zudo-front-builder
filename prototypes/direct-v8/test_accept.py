#!/usr/bin/env python3
"""Focused refusal coverage for accept.py's executable verification.

Runs with `python3 -I prototypes/direct-v8/test_accept.py` from the
repository root. No Cargo build, no second target directory: the runners are
tiny shell scripts standing in for the real executables, so the test proves
the refusal logic, not any engine behavior.
"""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()
