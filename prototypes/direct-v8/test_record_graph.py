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
        self.assertEqual(argv[argv.index("--target") + 1], "aarch64-apple-darwin")
        self.assertEqual((report["target"], report["features"]), ("aarch64-apple-darwin", "direct-v8,rolldown-prototype"))
        self.assertIn("aarch64-apple-darwin", report["build_command"])

    def test_default_target_is_linux_x64(self):
        _, report, argv = self.run_with("\n".join(CLEAN) + "\n", "--cli")
        self.assertEqual(argv[argv.index("--target") + 1], "x86_64-unknown-linux-gnu")
        self.assertEqual(report["features"], "direct-v8")

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
