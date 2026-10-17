#!/usr/bin/env python3
"""Report-consistency regressions, not new native/browser evidence.

Success fixtures are intentionally synthetic. Mutation matrices use the real
gate reader with an in-process report writer. A bounded subprocess pair also
checks the execution boundary; no Cargo, V8 or browser process is exercised.
"""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("accept_reports_subject", HERE / "accept.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)

BASELINE = "deno_core 0.399.0 / v8 147.4.0"
CANDIDATE = "direct v8 147.4.0"


def rows(*names):
    return [{"name": name, "passed": True} for name in names]


def engines():
    return {"results": [
        {"engine": engine, "passed": True, "exit_code": 0, "receipt_count": 2,
         "expected_count": 2, "rows": rows("first", "second")}
        for engine in (BASELINE, CANDIDATE)]}


def generated():
    return {"passed": True, "snapshot_manifest_sha256": "a" * 64,
            "baseline": {"engine": BASELINE, "passed": True,
                         "rows": rows("emitted-config", "render", "authored-mapped-throw")},
            "candidate": {"engine": CANDIDATE, "passed": True,
                          "rows": rows("emitted-config", "render", "authored-mapped-throw")},
            "byte_and_value_parity": rows("emitted-config", "render"),
            "browser": {"passed": True, "exit_code": 0}}


def http():
    return {"passed": True, "rows": [
        {"name": "loopback", "passed": True, "results": [
            {"engine": engine, "passed": True, "exit_code": 0}
            for engine in (BASELINE, CANDIDATE)]}]}


def outer():
    return {"passed": True, "exit_code": 0,
            "requests": [{"name": "object"}, {"name": "typed-field"}],
            "rows": rows("object", "typed-field")}


class ReportConsistencyTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.acceptance = accept.Acceptance(SimpleNamespace(output=self.root, guard=None))
        self.acceptance.logs.mkdir()

    def gate(self, report, counter, *, code=0, real=False):
        output = self.root / "report.json"
        checker = self.root / "checker.py"
        payload = json.dumps(report)
        checker.write_text("from pathlib import Path\nimport sys\n" +
                           f"Path({str(output)!r}).write_text({payload!r})\nsys.exit({code})\n")
        def write_report(*_args, **_kwargs):
            output.write_text(payload)
            if code:
                raise accept.Refusal(f"report-fixture failed (exit {code})")
            return SimpleNamespace(returncode=0)

        execution = contextlib.nullcontext() if real else mock.patch.object(
            self.acceptance, "run", side_effect=write_report)
        with execution:
            result = self.acceptance.gate("report-fixture", [str(checker)], output, counter)
        self.assertEqual(json.loads(output.read_text()), report, "raw evidence must not be rewritten")
        return result

    def reject(self, report, counter):
        result = self.gate(report, counter)
        self.assertIs(result["passed"], False)
        self.assertIsNone(result["counts"])
        self.assertIn("invalid gate report", " ".join(result["errors"]))
        return result

    def test_bare_passing_flag_is_not_evidence_in_any_report_format(self):
        for counter in (accept.engine_counts, accept.rows_counts, accept.generated_counts):
            with self.subTest(counter=counter.__name__):
                self.reject({"passed": True}, counter)

    def test_passing_rows_must_be_nonempty_named_unique_and_boolean(self):
        for values in ([], [None], [{}], [{"name": "one"}], rows(""), rows("one", "one"),
                       [{"name": "one", "passed": 1}], [{"name": "one", "passed": "yes"}],
                       [{"name": "one", "passed": False}]):
            with self.subTest(rows=values):
                self.reject({"passed": True, "rows": values}, accept.rows_counts)

    def test_engine_success_requires_exactly_one_of_each_engine(self):
        for selection in ([], [0], [1], [0, 0], [0, 1, 1]):
            with self.subTest(selection=selection):
                source = engines()["results"]
                self.reject({"passed": True, "results": [source[i] for i in selection]}, accept.engine_counts)
        for name in (None, "wrong engine"):
            report = engines()
            report["results"][1]["engine"] = name
            self.reject(report, accept.engine_counts)

    def test_engine_counts_exit_and_rows_must_agree(self):
        mutations = [("exit_code", value) for value in (None, False, 1, "0")]
        mutations += [("expected_count", value) for value in (None, True, 0, -1, 1, "2", 2.0)]
        mutations += [("receipt_count", value) for value in (None, True, 0, 1, 3, "2", 2.0)]
        mutations += [("rows", []), ("rows", rows("first")), ("rows", rows("first", "first"))]
        for field, value in mutations:
            with self.subTest(field=field, value=value):
                report = engines()
                report["results"][1][field] = value
                self.reject(report, accept.engine_counts)

    def test_matching_counts_cannot_hide_mismatched_case_names(self):
        report = engines()
        report["results"][1]["rows"] = rows("first", "different")
        self.reject(report, accept.engine_counts)

    def test_top_level_success_cannot_hide_failed_engine_or_row(self):
        report = engines()
        report["passed"] = True
        report["results"][1]["passed"] = False
        self.reject(report, accept.engine_counts)
        report = engines()
        report["results"][0]["rows"][0]["passed"] = False
        self.reject(report, accept.engine_counts)

    def test_generated_requires_both_named_engines_and_their_rows(self):
        for side in ("baseline", "candidate"):
            for field, value in (("engine", "wrong"), ("passed", False), ("passed", 1),
                                 ("rows", []), ("rows", rows("emitted-config"))):
                with self.subTest(side=side, field=field, value=value):
                    report = generated()
                    report[side][field] = value
                    self.reject(report, accept.generated_counts)
            report = generated()
            del report[side]
            self.reject(report, accept.generated_counts)

    def test_generated_parity_and_mapped_throw_cannot_be_dropped_or_substituted(self):
        for parity in ([], rows("emitted-config"), rows("emitted-config", "render", "extra"),
                       rows("emitted-config", "emitted-config"), rows("emitted-config", "other")):
            with self.subTest(parity=parity):
                report = generated()
                report["byte_and_value_parity"] = parity
                self.reject(report, accept.generated_counts)
        report = generated()
        for side in ("baseline", "candidate"):
            report[side]["rows"].pop()
        self.reject(report, accept.generated_counts)

    def test_generated_nested_row_failures_cannot_hide_under_passing_flags(self):
        for part in ("baseline", "candidate", "byte_and_value_parity"):
            for flag in (False, 1, "true"):
                with self.subTest(part=part, flag=flag):
                    report = generated()
                    collection = report[part] if part == "byte_and_value_parity" else report[part]["rows"]
                    collection[0]["passed"] = flag
                    self.reject(report, accept.generated_counts)

    def test_generated_requires_real_browser_success_not_skipped_or_failed(self):
        for browser in (None, {}, {"passed": False}, {"passed": 1}, {"passed": True},
                        {"passed": True, "exit_code": False}, {"passed": True, "exit_code": 7},
                        {"passed": True, "exit_code": 0, "not_run": "runtime assertions failed"}):
            with self.subTest(browser=browser):
                report = generated()
                report["browser"] = browser
                self.reject(report, accept.generated_counts)

    def test_generated_requires_snapshot_identity(self):
        for digest in (None, "", "bad", "g" * 64, 42):
            with self.subTest(digest=digest):
                report = generated()
                report["snapshot_manifest_sha256"] = digest
                self.reject(report, accept.generated_counts)

    def test_valid_success_and_counts_survive_for_all_formats(self):
        fixtures = [(engines(), accept.engine_counts), ({"passed": True, "rows": rows("row")}, accept.rows_counts),
                    (generated(), accept.generated_counts)]
        for report, counter in fixtures:
            with self.subTest(counter=counter.__name__):
                before = copy.deepcopy(report)
                result = self.gate(report, counter)
                self.assertIs(result["passed"], True)
                self.assertNotIn("errors", result)
                self.assertEqual(report, before)
        result = accept.generated_counts(generated())
        self.assertEqual(result["baseline"], {"packets": 3, "packets_passed": 3})
        self.assertEqual(result["parity_passed"], 2)
        self.assertIs(result["browser"], True)

    def test_failure_reports_keep_partial_counts_without_becoming_passes(self):
        for report, counter in (({"passed": False, "rows": [{"passed": False}]}, accept.rows_counts),
                                ({"results": [{"engine": CANDIDATE, "passed": False, "watchdog": "expired"}]},
                                 accept.engine_counts),
                                ({"passed": False, "error": "producer failed"}, accept.generated_counts)):
            with self.subTest(counter=counter.__name__):
                result = self.gate(report, counter)
                self.assertIs(result["passed"], False)
                self.assertIsNotNone(result["counts"])
                self.assertTrue(result["errors"])

    def test_failed_commands_keep_both_errors_on_inconsistent_success_reports(self):
        result = self.gate({"passed": True, "rows": []}, accept.rows_counts, code=7)
        self.assertIs(result["passed"], False)
        self.assertEqual(len(result["errors"]), 2)
        self.assertIn("exit 7", result["errors"][0])
        self.assertIn("invalid gate report", result["errors"][1])

    def test_real_subprocess_report_is_validated_after_a_zero_exit(self):
        for report, expected in ((generated(), True), ({"passed": True}, False)):
            with self.subTest(expected=expected):
                result = self.gate(report, accept.generated_counts, real=True)
                self.assertIs(result["passed"], expected)
                self.assertEqual(self.acceptance.steps[-1]["exit_code"], 0)

    def test_http_requires_a_successful_engine_pair_in_every_row(self):
        self.assertTrue(self.gate(http(), accept.http_counts)["passed"])
        for results in (None, [], http()["rows"][0]["results"][:1]):
            with self.subTest(results=results):
                report = http()
                report["rows"][0]["results"] = results
                self.reject(report, accept.http_counts)
        for field, value in (("engine", BASELINE), ("passed", False), ("exit_code", False), ("exit_code", 7)):
            with self.subTest(field=field, value=value):
                report = http()
                report["rows"][0]["results"][1][field] = value
                self.reject(report, accept.http_counts)

    def test_outer_requires_matching_request_rows_and_a_successful_exit(self):
        self.assertTrue(self.gate(outer(), accept.outer_counts)["passed"])
        for field, value in (("requests", None), ("requests", []), ("requests", [{"name": "object"}]),
                             ("requests", [{"name": "typed-field"}, {"name": "object"}]),
                             ("exit_code", None), ("exit_code", False), ("exit_code", 7)):
            with self.subTest(field=field, value=value):
                report = outer()
                report[field] = value
                self.reject(report, accept.outer_counts)

    def test_failure_counting_does_not_count_truthy_nonbooleans_as_passes(self):
        report = {"passed": False, "rows": [{"passed": 1}, {"passed": "yes"}, {"passed": True}]}
        self.assertEqual(accept.rows_counts(report)["rows_passed"], 1)


class MainReportConsistencyTest(unittest.TestCase):
    """Keep main/gate/counter/receipt wiring real; mock native execution only."""

    def run_acceptance(self, *, include_generated=True, corrupt=None):
        with tempfile.TemporaryDirectory() as directory:
            # accept.ROOT is canonical in production; macOS TMPDIR is a /var -> /private/var alias.
            root = Path(directory).resolve()
            target = root / "target"
            examples = target / "debug" / "examples"
            examples.mkdir(parents=True)
            paths = {name: examples / name for name in
                     ("direct_v8_baseline", "direct_v8", "direct_v8_outer_config", "direct_v8_postprocess")}
            paths["zfb"] = target / "debug" / "zfb"
            paths["wrapper"] = target / "debug" / "wrapper"
            for path in paths.values():
                path.write_text("#!/bin/sh\nexit 0\n")
                path.chmod(0o755)
            reports = {"gate-config": engines(), "gate-services": engines(), "gate-modules": engines(),
                       "gate-http": http(), "gate-outer-config": outer(), "gate-generated": generated()}
            if corrupt:
                corrupt(reports)

            def run(instance, label, argv, **_kwargs):
                instance.steps.append({"label": label, "exit_code": 0, "passed": True})
                stdout = ""
                if label in ("services-tests", "candidate-lib-tests"):
                    stdout = "test result: ok. 1 passed; 0 failed;"
                elif label == "build-wrapper-test":
                    stdout = json.dumps({"reason": "compiler-artifact", "target": {"kind": ["lib"], "name": "zfb"},
                                         "profile": {"test": True}, "executable": str(paths["wrapper"])})
                elif label in reports:
                    output = Path(argv[argv.index("--output") + 1])
                    if label == "gate-generated":
                        output.mkdir()
                        output /= "results.json"
                    output.write_text(json.dumps(reports[label]))
                return SimpleNamespace(stdout=stdout, args=argv)

            def probe(path, expected_engine, label):
                return accept.record_executable(path, label) | {"engine": expected_engine}

            output = target / "acceptance-test"
            argv = ["accept.py", "--output", str(output)]
            if include_generated:
                argv += ["--generated", "--esbuild", "/unused/esbuild", "--chromium", "/unused/chromium",
                         "--node", "/unused/node", "--rustc", "/unused/rustc"]
            with mock.patch.object(accept, "ROOT", root), \
                    mock.patch.object(accept.Acceptance, "environment", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "prepare_icu", return_value={"fixture": True}), \
                    mock.patch.object(accept.Acceptance, "cargo_target_dir", return_value=target), \
                    mock.patch.object(accept.Acceptance, "run", new=run), \
                    mock.patch.object(accept, "verify_runner", side_effect=probe), \
                    mock.patch.object(accept.sys, "argv", argv), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                status = accept.main()
            return status, json.loads((output / "acceptance.json").read_text())

    def test_complete_reports_pass_in_both_modes_with_existing_count_shapes(self):
        for include_generated in (False, True):
            with self.subTest(include_generated=include_generated):
                status, report = self.run_acceptance(include_generated=include_generated)
                self.assertEqual(status, 0)
                self.assertTrue(report["passed"])
                self.assertEqual(len(report["gates"]), 6 if include_generated else 5)
                self.assertEqual(report["gates"]["http"]["counts"]["rows_passed"], 1)
                self.assertEqual(report["gates"]["outer_config"]["counts"]["rows_passed"], 2)
                self.assertEqual(report["gates"]["config"]["counts"][1]["expected"], 2)

    def test_every_gate_rejects_bare_success_in_the_final_acceptance_receipt(self):
        for label, key in (("gate-config", "config"), ("gate-services", "services"), ("gate-http", "http"),
                           ("gate-modules", "modules"), ("gate-outer-config", "outer_config"),
                           ("gate-generated", "generated")):
            with self.subTest(label=label):
                status, report = self.run_acceptance(corrupt=lambda reports: reports.update({label: {"passed": True}}))
                self.assertEqual(status, 1)
                self.assertFalse(report["passed"])
                self.assertFalse(report["gates"][key]["passed"])
                self.assertTrue(report["gates"][key]["errors"])
                self.assertEqual(report["error"], f"gates failed: {key}")
                # Later independent gates and final executable checks do not
                # erase the earlier refusal, and their observations survive.
                self.assertEqual(len(report["gates"]), 6)
                self.assertTrue(report["executable_checks"][-1]["passed"])

    def test_http_and_outer_use_the_specialized_validators(self):
        def missing_http(reports):
            del reports["gate-http"]["rows"][0]["results"]

        def missing_outer(reports):
            del reports["gate-outer-config"]["requests"]

        for corrupt, key in ((missing_http, "http"), (missing_outer, "outer_config")):
            with self.subTest(key=key):
                status, report = self.run_acceptance(corrupt=corrupt)
                self.assertEqual(status, 1)
                self.assertEqual(report["error"], f"gates failed: {key}")


if __name__ == "__main__":
    unittest.main()
