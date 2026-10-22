#!/usr/bin/env python3
"""Offline graph-classifier controls with a fake cargo; not a real dependency resolution."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("record-graph.py")
spec = importlib.util.spec_from_file_location("record_graph", SCRIPT)
graph = importlib.util.module_from_spec(spec)
spec.loader.exec_module(graph)
ENGINE = "v8 v147.4.0 features=[default,use_custom_libcxx]"
CLEAN = ["zfb v0.0.0 features=[direct-v8]", "zfb-render v0.0.0 features=[direct-v8]", ENGINE]


class Classifier(unittest.TestCase):
    def test_clean_cli_closure_passes(self):
        self.assertEqual(graph.evaluate(CLEAN, cli=True), (True, [], [ENGINE]))

    def test_standalone_rejects_cli_crates(self):
        passed, invalid, _ = graph.evaluate(CLEAN, cli=False)
        self.assertFalse(passed)
        self.assertEqual(invalid, ["zfb-render v0.0.0 features=[direct-v8]"])

    def test_cli_forbids_only_deno_and_serde_v8_records(self):
        for row in ("zfb-render v0.0.0 features=[]", "zfb-build v0.0.0 features=[]", "zfb-js-host v0.0.0 features=[]"):
            self.assertTrue(graph.evaluate([*CLEAN, row], cli=True)[0], row)
        for row in ("zfb-render v0.0.0 features=[]", "zfb-build v0.0.0 features=[]"):
            self.assertFalse(graph.evaluate([*CLEAN, row], cli=False)[0], row)

    def test_any_deno_crate_rejects(self):
        for row in ["deno_core v0.400.0 features=[]", "deno_unsync v0.4.0 features=[]", "serde_v8 v0.300.0 features=[]"]:
            self.assertFalse(graph.evaluate([*CLEAN, row], cli=True)[0], row)

    def test_engine_must_be_single_pinned_v8(self):
        self.assertFalse(graph.evaluate(CLEAN[:2], cli=True)[0])
        self.assertFalse(graph.evaluate([*CLEAN[:2], "v8 v146.0.0 features=[]"], cli=True)[0])
        self.assertFalse(graph.evaluate([*CLEAN, "v8 v146.0.0 features=[]"], cli=True)[0])


class Command(unittest.TestCase):
    def run_with(self, tree, *argv):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            cargo = tmp / "cargo"
            log = tmp / "argv.json"
            cargo.write_text("#!/usr/bin/env python3\nimport json, sys\n"
                             f"json.dump(sys.argv[1:], open({str(log)!r}, 'w'))\n"
                             f"sys.stdout.write({tree!r})\n")
            cargo.chmod(0o755)
            out = tmp / "report.json"
            result = subprocess.run([sys.executable, str(SCRIPT), "--cargo", str(cargo), "--output", str(out), *argv],
                                    capture_output=True, text=True)
            report = json.loads(out.read_text()) if out.exists() else None
            return result, report, json.loads(log.read_text()) if log.exists() else None

    def test_target_and_extra_feature_reach_cargo_and_report(self):
        result, report, argv = self.run_with("\n".join(CLEAN) + "\n", "--cli", "--target", "aarch64-apple-darwin",
                                             "--extra-feature", "rolldown-prototype")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("direct-v8,rolldown-prototype", argv)
        self.assertEqual(argv[argv.index("--color") + 1], "never")
        self.assertEqual(argv[argv.index("--target") + 1], "aarch64-apple-darwin")
        self.assertEqual((report["target"], report["features"]), ("aarch64-apple-darwin", "direct-v8,rolldown-prototype"))
        self.assertIn("aarch64-apple-darwin", report["build_command"])

    def test_default_target_is_linux_x64(self):
        _, report, argv = self.run_with("\n".join(CLEAN) + "\n", "--cli")
        self.assertEqual(argv[argv.index("--target") + 1], "x86_64-unknown-linux-gnu")
        self.assertEqual(report["features"], "direct-v8")

    def test_default_selection_passes_no_feature_flags_and_records_selection(self):
        result, report, argv = self.run_with("\n".join(CLEAN) + "\n", "--cli", "--selection", "default",
                                             "--target", "x86_64-apple-darwin")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("--no-default-features", argv)
        self.assertNotIn("--features", argv)
        self.assertEqual(argv[argv.index("-p") + 1], "zfb")
        self.assertEqual((report["selection"], report["features"]), ("default", "default"))
        self.assertNotIn("--no-default-features", report["build_command"])
        self.assertEqual(report["build_command"][-2:], ["--bin", "zfb"])

    def test_direct_v8_selection_is_explicit_and_recorded(self):
        _, report, argv = self.run_with("\n".join(CLEAN) + "\n", "--cli")
        self.assertEqual(report["selection"], "direct-v8")
        self.assertEqual(argv[argv.index("--features") + 1], "direct-v8")
        self.assertIn("--no-default-features", argv)

    def test_default_selection_that_still_resolves_deno_fails_honestly(self):
        tree = "\n".join([*CLEAN[:2], "deno_core v0.400.0 features=[]", "v8 v146.0.0 features=[]"]) + "\n"
        result, report, _ = self.run_with(tree, "--cli", "--selection", "default")
        self.assertEqual(result.returncode, 1)
        self.assertTrue(result.stdout.startswith("FAIL"))
        self.assertFalse(report["passed"])
        self.assertEqual(report["forbidden_dependencies"], ["deno_core v0.400.0 features=[]"])

    def test_default_selection_requires_cli_and_no_extra_feature(self):
        for argv in (["--selection", "default"], ["--cli", "--selection", "default", "--extra-feature", "rolldown-prototype"],
                     ["--cli", "--selection", "other"]):
            result, report, cargo_argv = self.run_with("", *argv)
            self.assertEqual(result.returncode, 2, argv)
            self.assertIsNone(report)
            self.assertIsNone(cargo_argv)

    def test_deno_row_fails_with_report(self):
        result, report, _ = self.run_with("\n".join([*CLEAN, "deno_core v0.400.0 features=[]"]) + "\n", "--cli")
        self.assertEqual(result.returncode, 1)
        self.assertFalse(report["passed"])

    def test_unknown_target_and_standalone_extra_feature_are_refused(self):
        for argv in (["--cli", "--target", "x86_64-unknown-linux-musl"], ["--extra-feature", "rolldown-prototype"]):
            result, report, cargo_argv = self.run_with("", *argv)
            self.assertEqual(result.returncode, 2, argv)
            self.assertIsNone(report)
            self.assertIsNone(cargo_argv)


if __name__ == "__main__":
    unittest.main()
