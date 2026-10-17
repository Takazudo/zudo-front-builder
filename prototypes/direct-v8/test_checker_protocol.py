#!/usr/bin/env python3
"""Config-checker CLI regressions using real, tiny stand-in subprocesses.

Run: python3 -I -B -m unittest discover -s prototypes/direct-v8 -p 'test_checker*.py'
These tests exercise Python checkers and wire validation, never native V8.
"""
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
BASELINE = "deno_core 0.399.0 / v8 147.4.0"
CANDIDATE = "direct v8 147.4.0"
HOST_PREFIX = "ZFB_HOST_RECEIPT "
OUTER_PREFIX = "ZFB_OUTER_CONFIG_RECEIPT "


def framed(prefix, documents):
    return "".join(prefix + json.dumps(doc) + "\n" for doc in documents).encode()


def outer_receipts():
    return [
        {"case": "object", "ok": True, "outDir": "dist"},
        {"case": "empty-array", "ok": True, "outDir": "dist"},
        {"case": "outdir-array", "ok": True, "outDir": "array-output"},
        {"case": "wrong-array", "ok": False, "error": "[0]: expected path string"},
        {"case": "date", "ok": False, "error": "expected struct Config"},
        {"case": "typed-field", "ok": False, "error": "port: expected u16"},
        {"case": "presets", "ok": True, "outDir": "user-output"},
    ]


class CheckerProtocolTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.sequence = 0
        self.cases = [{"name": "literal", "contracts": ["FIXTURE"],
                       "source": "export default {probe: 42};",
                       "expected": {"ok": True, "value": {"probe": 42}}}]
        self.corpus = self.root / "cases.json"
        self.write_corpus()

    def write_corpus(self):
        self.corpus.write_text(json.dumps(self.cases) + "\n")

    def runner(self, output, *, exit_code=0, extra="", capture=None):
        self.sequence += 1
        path = self.root / f"runner-{self.sequence}"
        capture_source = f"Path({str(capture)!r}).write_bytes(wire)\n" if capture else ""
        path.write_text(f"#!{sys.executable} -S\nimport os, sys\nfrom pathlib import Path\n"
                        f"wire = sys.stdin.buffer.read()\n{capture_source}os.write(1, {output!r})\n"
                        f"os.write(2, b'fixture stderr\\n')\n{extra}\nsys.exit({exit_code})\n")
        path.chmod(0o755)
        return path

    def invoke(self, checker, args):
        self.sequence += 1
        output = self.root / f"report-{self.sequence}.json"
        process = subprocess.run([sys.executable, "-I", "-B", "-S", str(HERE / checker),
                                  *map(str, args), "--output", str(output)],
                                 capture_output=True, text=True, timeout=10, check=False)
        self.assertTrue(output.is_file(), f"missing failure report; exit={process.returncode}: {process.stderr}")
        return process, json.loads(output.read_text())

    def config(self, payload, *, expected_ok=True, exit_code=0, extra="", candidate=True):
        if not expected_ok:
            self.cases[0]["expected"] = {"ok": False, "contains": ["sentinel"]}
            self.write_corpus()
        good = {"engine": BASELINE, **self.cases[0]["expected"]}
        if not expected_ok:
            good = {"engine": BASELINE, "ok": False, "error": "sentinel"}
        baseline = self.runner(framed(HOST_PREFIX, [good]))
        subject = self.runner(payload, exit_code=exit_code, extra=extra)
        args = ["--baseline", baseline, "--cases", self.corpus]
        if candidate:
            args += ["--candidate", subject]
        else:
            args[1] = subject
        return self.invoke("check-config.py", args)

    def outer(self, payload, *, exit_code=0):
        return self.invoke("check-outer-config.py", ["--runner", self.runner(payload, exit_code=exit_code)])

    def test_existing_positive_protocols_pass(self):
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}]))
        self.assertEqual(proc.returncode, 0)
        self.assertEqual([r["engine"] for r in report["results"]], [BASELINE, CANDIDATE])
        self.assertTrue(all(r["passed"] for r in report["results"]))
        proc, report = self.outer(framed(OUTER_PREFIX, outer_receipts()))
        self.assertEqual(proc.returncode, 0)
        self.assertTrue(report["passed"])
        self.assertEqual(len(report["rows"]), 7)

    def test_numeric_success_or_failure_flags_cannot_pass_config(self):
        for ok, value in ((True, 1), (False, 0)):
            with self.subTest(ok=ok):
                doc = {"engine": CANDIDATE, "ok": value, "value": {"probe": 42}, "error": "sentinel"}
                proc, report = self.config(framed(HOST_PREFIX, [doc]), expected_ok=ok)
                self.assertEqual(proc.returncode, 1)
                self.assertFalse(report["results"][1]["passed"])

    def test_missing_value_is_not_a_successful_null(self):
        self.cases[0]["expected"]["value"] = None
        self.write_corpus()
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True}]))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][1]["passed"])

    def test_extra_outer_receipt_is_not_silently_truncated(self):
        rows = outer_receipts()
        proc, report = self.outer(framed(OUTER_PREFIX, rows + [rows[0]]))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertEqual(report["receipt_count"], 8)
        self.assertEqual(report["expected_count"], 7)

    def test_missing_receipts_keep_all_expected_failure_rows(self):
        proc, report = self.config(b"")
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(len(report["results"][1]["rows"]), 1)
        self.assertFalse(report["results"][1]["rows"][0]["passed"])
        proc, report = self.outer(framed(OUTER_PREFIX, outer_receipts()[:1]))
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(len(report["rows"]), 7)
        self.assertTrue(report["rows"][0]["passed"])
        self.assertTrue(all(not row["passed"] for row in report["rows"][1:]))

    def test_malformed_and_nonobject_receipts_save_both_engine_observations(self):
        for payload in (b'{"ok":', b'null', b'[]', b'42'):
            with self.subTest(payload=payload):
                malformed = HOST_PREFIX.encode() + payload + b"\n"
                proc, report = self.config(malformed)
                self.assertEqual(proc.returncode, 1)
                self.assertTrue(report["results"][0]["passed"])
                self.assertFalse(report["results"][1]["passed"])
                self.assertIn(payload.decode(), report["results"][1]["stdout"])

    def test_invalid_outer_json_keeps_positions_and_later_rows(self):
        good = outer_receipts()
        payload = framed(OUTER_PREFIX, good[:1]) + OUTER_PREFIX.encode() + b"null\n" + framed(OUTER_PREFIX, good[2:])
        proc, report = self.outer(payload)
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(len(report["rows"]), 7)
        self.assertFalse(report["rows"][1]["passed"])
        self.assertTrue(report["rows"][2]["passed"])
        self.assertTrue(report["rows"][-1]["passed"])

    def test_duplicate_json_keys_and_nonfinite_numbers_are_rejected(self):
        for suffix in ('"ok":false,"ok":true,"value":{"probe":42}',
                       '"ok":true,"value":{"probe":42},"extra":NaN',
                       '"ok":true,"value":{"probe":0,"probe":42}'):
            with self.subTest(suffix=suffix):
                payload = (HOST_PREFIX + '{"engine":' + json.dumps(CANDIDATE) + ',' + suffix + '}\n').encode()
                proc, report = self.config(payload)
                self.assertEqual(proc.returncode, 1)
                self.assertFalse(report["results"][1]["passed"])

    def test_invalid_utf8_is_retained_and_cannot_pass(self):
        payload = b"console: \xff\n" + framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}])
        proc, report = self.config(payload)
        self.assertEqual(proc.returncode, 1)
        result = report["results"][1]
        self.assertFalse(result["passed"])
        self.assertEqual(base64.b64decode(result["stdout_base64"]), payload)

    def test_error_payload_must_be_a_string_not_an_array_of_fragments(self):
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": False, "error": ["sentinel"]}]), expected_ok=False)
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][1]["passed"])
        rows = outer_receipts()
        rows[3]["error"] = ["[0]", "expected path string"]
        proc, report = self.outer(framed(OUTER_PREFIX, rows))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])

    def test_nonzero_exit_cannot_be_rescued_by_valid_rows(self):
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}]), exit_code=7)
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(report["results"][1]["exit_code"], 7)
        self.assertFalse(report["results"][1]["passed"])
        proc, report = self.outer(framed(OUTER_PREFIX, outer_receipts()), exit_code=7)
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])

    def test_missing_baseline_does_not_hide_candidate_observation(self):
        candidate = self.runner(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}]))
        proc, report = self.invoke("check-config.py", ["--baseline", self.root / "absent", "--candidate", candidate, "--cases", self.corpus])
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][0]["passed"])
        self.assertTrue(report["results"][1]["passed"])
        self.assertIsNone(report["results"][0]["exit_code"])

    def test_missing_outer_runner_writes_a_failure_report(self):
        proc, report = self.invoke("check-outer-config.py", ["--runner", self.root / "absent"])
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertIsNone(report["exit_code"])

    def test_empty_corpus_refuses_before_running_a_runner(self):
        self.cases = []
        self.write_corpus()
        marker = self.root / "launched"
        runner = self.runner(b"", extra=f"Path({str(marker)!r}).touch()")
        proc, report = self.invoke("check-config.py", ["--baseline", runner, "--cases", self.corpus])
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertFalse(marker.exists())

    def test_corpus_hash_binds_loaded_bytes_and_mutation_fails(self):
        original_hash = hashlib.sha256(self.corpus.read_bytes()).hexdigest()
        payload = framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}])
        proc, report = self.config(payload, extra=f"Path({str(self.corpus)!r}).write_text('[]')")
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(report["corpus_sha256"], original_hash)
        self.assertFalse(report["passed"])
        self.assertTrue(all(result["passed"] for result in report["results"]))

    def test_deleted_executable_does_not_erase_a_failure_report(self):
        payload = framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}])
        proc, report = self.config(payload, extra="Path(__file__).unlink()")
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][1]["passed"])
        self.assertEqual(report["results"][1]["exit_code"], 0)


    def load(self, name):
        spec = importlib.util.spec_from_file_location("checker_test_subject", HERE / name)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_service_mode_and_frozen_wire_reach_both_processes(self):
        self.cases[0]["mode"] = "build-time"
        self.write_corpus()
        original_hash = hashlib.sha256(self.corpus.read_bytes()).hexdigest()
        captures = [self.root / "baseline-input", self.root / "candidate-input"]
        runners = [self.runner(framed(HOST_PREFIX, [{"engine": engine, "ok": True, "value": {"probe": 42}}]),
                               capture=capture,
                               extra=f"Path({str(self.corpus)!r}).write_text('[]')" if index == 0 else "")
                   for index, (engine, capture) in enumerate(zip((BASELINE, CANDIDATE), captures))]
        proc, report = self.invoke("check-config.py", ["--baseline", runners[0], "--candidate", runners[1],
                                                      "--kind", "script", "--cases", self.corpus])
        self.assertEqual(proc.returncode, 1)  # The input file changed, even though both engines agreed.
        self.assertEqual(report["corpus_sha256"], original_hash)
        self.assertEqual(captures[0].read_bytes(), captures[1].read_bytes())
        self.assertEqual(json.loads(captures[1].read_text()),
                         {"kind": "script", "source": self.cases[0]["source"], "mode": "build-time"})
        self.assertTrue(all(result["passed"] for result in report["results"]))

    def test_baseline_only_mode_and_console_noise_remain_supported(self):
        payload = b"ordinary console output\n" + framed(HOST_PREFIX, [{"engine": BASELINE, "ok": True, "value": {"probe": 42}}])
        proc, report = self.config(payload, candidate=False)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(len(report["results"]), 1)
        self.assertTrue(report["passed"])

    def test_explicit_null_is_valid_but_boolean_is_not_integer_value(self):
        self.cases[0]["expected"]["value"] = None
        self.write_corpus()
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": None}]))
        self.assertEqual(proc.returncode, 0)
        self.assertTrue(report["passed"])
        self.cases[0]["expected"]["value"] = {"probe": 1}
        self.write_corpus()
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": True}}]))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][1]["passed"])

    def test_unicode_line_separators_inside_json_are_not_frame_boundaries(self):
        value = "first\u0085second\u2028third\u2029fourth"
        self.cases[0]["expected"]["value"] = value
        self.write_corpus()
        payload = (HOST_PREFIX + json.dumps({"engine": CANDIDATE, "ok": True, "value": value}, ensure_ascii=False) + "\n").encode()
        proc, report = self.config(payload)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(report["results"][1]["receipt_count"], 1)

    def test_wrong_engine_and_reordered_outer_results_are_rejected(self):
        proc, report = self.config(framed(HOST_PREFIX, [{"engine": BASELINE, "ok": True, "value": {"probe": 42}}]))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][1]["passed"])
        rows = outer_receipts()
        rows[0], rows[1] = rows[1], rows[0]
        proc, report = self.outer(framed(OUTER_PREFIX, rows))
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["rows"][0]["passed"])
        self.assertFalse(report["rows"][1]["passed"])

    def test_invalid_corpus_never_launches_either_engine(self):
        marker = self.root / "launched"
        runner = self.runner(b"", extra=f"Path({str(marker)!r}).touch()")
        invalid = [b"{", b"null", b"[null]", json.dumps(self.cases * 2).encode(),
                   json.dumps([{**self.cases[0], "expected": {"ok": 1, "value": 42}}]).encode(),
                   json.dumps([{**self.cases[0], "expected": {"ok": True}}]).encode()]
        for contents in invalid:
            with self.subTest(contents=contents):
                self.corpus.write_bytes(contents)
                proc, report = self.invoke("check-config.py", ["--baseline", runner, "--candidate", runner, "--cases", self.corpus])
                self.assertEqual(proc.returncode, 1)
                self.assertFalse(report["passed"])
                self.assertEqual(report["results"], [])
                self.assertFalse(marker.exists())

    def test_real_watchdog_preserves_partial_bytes_and_failed_outer_rows(self):
        outer = self.load("check-outer-config.py")
        payload = b"partial \xf0\x9f"
        runner = self.runner(payload, extra="import time; time.sleep(30)")
        report = outer.run(runner, timeout=0.3)
        self.assertFalse(report["passed"])
        self.assertIsNone(report["exit_code"])
        self.assertIn("watchdog", report["watchdog"])
        self.assertEqual(base64.b64decode(report["stdout_base64"]), payload)
        self.assertEqual(report["stderr"], "fixture stderr\n")
        self.assertEqual(len(report["rows"]), 7)
        self.assertTrue(all(not row["passed"] for row in report["rows"]))

    def test_changed_bytes_or_execute_permission_fail_after_zero_exit(self):
        payload = framed(HOST_PREFIX, [{"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}])
        for change in ("Path(__file__).write_text(Path(__file__).read_text() + '# changed')",
                       "Path(__file__).chmod(0o644)"):
            with self.subTest(change=change):
                proc, report = self.config(payload, extra=change)
                self.assertEqual(proc.returncode, 1)
                self.assertEqual(report["results"][1]["exit_code"], 0)
                self.assertFalse(report["results"][1]["passed"])
                self.assertTrue(report["results"][1]["failures"])

    def test_missing_interpreter_is_not_an_observed_process_exit(self):
        runner = self.root / "bad-interpreter"
        runner.write_text(f"#!{self.root}/absent-python\n")
        runner.chmod(0o755)
        proc, report = self.invoke("check-outer-config.py", ["--runner", runner])
        self.assertEqual(proc.returncode, 1)
        self.assertIsNone(report["exit_code"])
        self.assertIn("failed to start", " ".join(report["failures"]))

    def test_signal_exit_is_recorded_not_called_a_success(self):
        payload = framed(OUTER_PREFIX, outer_receipts())
        runner = self.runner(payload, extra="import signal; os.kill(os.getpid(), signal.SIGTERM)")
        proc, report = self.invoke("check-outer-config.py", ["--runner", runner])
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(report["exit_code"], -15)
        self.assertFalse(report["passed"])
        self.assertTrue(all(row["passed"] for row in report["rows"]))

    def test_json_float_overflow_is_not_a_finite_receipt(self):
        payload = (HOST_PREFIX + '{"engine":' + json.dumps(CANDIDATE) +
                   ',"ok":true,"value":{"probe":42},"extra":1e400}\n').encode()
        proc, report = self.config(payload)
        self.assertEqual(proc.returncode, 1)
        self.assertIn("non-finite", " ".join(report["results"][1]["failures"]))

    def test_original_semantic_error_category_and_fragments_still_pass(self):
        self.cases[0]["expected"] = {"ok": False, "category": "evaluation", "errorContains": ["sentinel"]}
        self.write_corpus()
        runners = [self.runner(framed(HOST_PREFIX, [{"engine": engine, "ok": False,
                                                     "error": "config evaluation error: sentinel"}]))
                   for engine in (BASELINE, CANDIDATE)]
        proc, report = self.invoke("check-config.py", ["--baseline", runners[0], "--candidate", runners[1], "--cases", self.corpus])
        self.assertEqual(proc.returncode, 0)
        self.assertTrue(report["passed"])

    def test_deep_baseline_receipt_keeps_failure_and_candidate_evidence(self):
        nested = "[" * 1100 + "0" + "]" * 1100
        payload = (HOST_PREFIX + '{"engine":' + json.dumps(BASELINE) +
                   ',"ok":true,"value":{"probe":42},"extra":' + nested + '}\n').encode()
        baseline = self.runner(payload)
        candidate = self.runner(framed(HOST_PREFIX, [
            {"engine": CANDIDATE, "ok": True, "value": {"probe": 42}}]))
        proc, report = self.invoke("check-config.py", ["--baseline", baseline, "--candidate", candidate,
                                                      "--cases", self.corpus])
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["results"][0]["passed"])
        self.assertIn("nesting", " ".join(report["results"][0]["failures"]))
        self.assertEqual(report["results"][0]["stdout"].encode(), payload)
        self.assertIsNone(report["results"][0]["rows"][0]["receipt"])
        self.assertTrue(report["results"][1]["passed"])

    def test_deep_outer_receipt_retains_slot_and_later_rows(self):
        nested = "[" * 1100 + "0" + "]" * 1100
        payload = (OUTER_PREFIX + '{"case":"object","ok":true,"outDir":"dist","extra":' +
                   nested + '}\n').encode() + framed(OUTER_PREFIX, outer_receipts()[1:])
        proc, report = self.outer(payload)
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertEqual(report["receipt_count"], 7)
        self.assertIsNone(report["rows"][0]["receipt"])
        self.assertTrue(all(row["passed"] for row in report["rows"][1:]))
        self.assertEqual(report["stdout"].encode(), payload)

    def test_deep_corpus_refuses_before_launch(self):
        nested = "[" * 1100 + "0" + "]" * 1100
        self.corpus.write_text('[{"name":"deep","contracts":[],"source":"export default {};",'
                               '"expected":{"ok":true,"value":' + nested + '}}]')
        marker = self.root / "launched"
        runner = self.runner(b"", extra=f"Path({str(marker)!r}).touch()")
        proc, report = self.invoke("check-config.py", ["--baseline", runner, "--candidate", runner,
                                                      "--cases", self.corpus])
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertIn("nesting", report["error"])
        self.assertEqual(report["results"], [])
        self.assertFalse(marker.exists())


if __name__ == "__main__":
    unittest.main()
