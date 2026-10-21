#!/usr/bin/env python3
"""Offline controls for check-cli-cases.py; the cases themselves need a zfb binary."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("check_cli_cases", HERE / "check-cli-cases.py")
cases = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cases)


class Rows(unittest.TestCase):
    def test_a_failing_case_is_recorded_and_the_next_still_runs(self):
        report = {"rows": []}
        rows = cases.Rows(report)
        rows.record("first", lambda: (_ for _ in ()).throw(AssertionError("broken")))
        rows.record("second", lambda: {"detail": 1})
        self.assertEqual(report["rows"], [
            {"name": "first", "passed": False, "error": "AssertionError: broken"},
            {"name": "second", "passed": True, "detail": 1},
        ])


class Summary(unittest.TestCase):
    def test_names_the_variant_and_every_row(self):
        report = {"rolldown_prototype": "1", "passed": False, "rows": [
            {"name": "a", "passed": True}, {"name": "b", "passed": False, "error": "AssertionError: x"}]}
        self.assertEqual(cases.summary(report),
                         "CLI cases (Rolldown): FAIL\n- a: True\n- b: False AssertionError: x")

    def test_lists_rows_not_run_without_counting_them(self):
        report = {"rolldown_prototype": None, "passed": True, "rows": [{"name": "a", "passed": True}],
                  "not_run": [{"name": "browser-hydration", "reason": "no --chromium given"}]}
        self.assertEqual(cases.summary(report), "CLI cases (default bundler): PASS\n- a: True\n"
                         "- browser-hydration: NOT RUN (no --chromium given)")


class SourceMap(unittest.TestCase):
    def test_vlq_decodes_signed_values(self):
        self.assertEqual(cases.vlq("AAgBC"), [0, 0, 16, 1])
        self.assertEqual(cases.vlq("D"), [-1])

    def test_position_is_the_last_segment_at_or_before_the_column(self):
        # Line 0: column 0 -> a.ts:1, column 4 -> b.ts:2.
        source_map = {"sources": ["a.ts", "b.ts"], "mappings": "AAAA,ICCE"}
        self.assertEqual(cases.original_position(source_map, 0, 2), ("a.ts", 1))
        self.assertEqual(cases.original_position(source_map, 0, 9), ("b.ts", 2))
        self.assertIsNone(cases.original_position(source_map, 3, 0))

    def test_element_span_brackets_the_marker(self):
        authored = "x\n<p\n  class={a}>\n  marker\n</p>\n"
        self.assertEqual(cases.element_span(authored, "marker", "<p", "</p>"), (2, 5))


class Fixture(unittest.TestCase):
    def test_site_copy_drops_expectations_and_adds_case_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = cases.fixture(Path(directory) / "site", extra={"pages/ssr.tsx": cases.SSR_PAGE})
            self.assertFalse((root / "expected.json").exists())
            self.assertTrue((root / "pages/wasm.tsx").is_file())
            self.assertIn("prerender = false", (root / "pages/ssr.tsx").read_text())

    def test_minimal_fixture_has_only_the_config_and_case_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = cases.fixture(Path(directory) / "alias", base=False, extra=cases.ALIAS_FILES)
            files = sorted(path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file())
            self.assertEqual(files, sorted(["zfb.config.ts", *cases.ALIAS_FILES]))


if __name__ == "__main__":
    unittest.main()
