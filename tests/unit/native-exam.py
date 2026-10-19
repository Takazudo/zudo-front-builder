import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("native_exam", ROOT / "scripts/native-exam.py")
exam = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exam)
FIXTURE = ROOT / "tests/fixtures/native-exam"
SHA = "a" * 40


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for lane in ["shard-1", "shard-2", "shard-3", "shard-4", "doctests"]:
            p = self.root / ("native-exam-" + lane)
            if lane == "doctests":
                p.mkdir()
                shutil.copy(FIXTURE / "doc-list.txt", p / "full-list.txt")
                shutil.copy(FIXTURE / "doc-run.txt", p / "run.log")
            else:
                shutil.copytree(FIXTURE / "capture" / lane, p)
            self.write(p / "metadata.json", {
                "schema": 1, "source": SHA, "run_id": "123", "attempt": "1", "retries": 0,
                "arch": "arm64", "os": "15.7", "rust_verbose": "rustc verbose fixture", "image": "macos15", "image_version": "fixture", "hashes": {"Cargo.lock": "f" * 64},
                "tools": {"rust": "rustc 1.99.1 (capture)", "node": "v22.22.2", "nextest": "cargo-nextest 0.9.140", "cargo": "cargo 1.99.1", "pnpm": "12.8.2", "npm": "11", "esbuild": "0.25.12"},
            })
            self.write(p / "result.json", {"lane": lane, "rc": 0, "completed": True})
            self.write(p / "timings.json", {"install_seconds": 1, "build_seconds": 2, "inventory_and_tests_seconds": 3})
            (p / "exit-code.txt").write_text("0\n")
        self.jobs = {"shards": "success", "doctests": "success"}

    def write(self, path, value):
        path.write_text(json.dumps(value))

    def change(self, file, edit, lane="shard-1"):
        p = self.root / ("native-exam-" + lane) / file
        data = exam.read_json(p)
        edit(data)
        self.write(p, data)

    def check(self):
        return exam.aggregate(self.root, SHA, "123", "1", self.jobs)

    def test_real_nextest_capture_passes(self):
        result = self.check()
        self.assertEqual(result["selected"], 16)
        self.assertEqual(result["ignored"], 1)
        self.assertEqual(result["doctests"], {"passed": 1, "ignored": 1})

    def test_missing_shard(self):
        shutil.rmtree(self.root / "native-exam-shard-4")
        with self.assertRaisesRegex(ValueError, "artifact"):
            self.check()

    def test_missing_doctests(self):
        shutil.rmtree(self.root / "native-exam-doctests")
        with self.assertRaises(ValueError):
            self.check()

    def test_duplicate_selected_test(self):
        a = self.root / "native-exam-shard-1"
        b = self.root / "native-exam-shard-2"
        shutil.copy(a / "selected.json", b / "selected.json")
        shutil.copy(a / "junit.xml", b / "junit.xml")
        with self.assertRaisesRegex(ValueError, "overlapping"):
            self.check()

    def test_missing_test_from_union(self):
        # Remove a selected case and its JUnit result consistently: local parity passes,
        # but the global nonignored inventory must still reject the omission.
        p = self.root / "native-exam-shard-1"
        data = exam.read_json(p / "selected.json")
        cases = data["rust-suites"]["native-probe"]["testcases"]
        name = next(n for n, c in cases.items() if c["filter-match"]["status"] == "matches")
        cases[name]["filter-match"] = {"status": "mismatch", "reason": "partition"}
        self.write(p / "selected.json", data)
        tree = exam.ET.parse(p / "junit.xml")
        suite = tree.getroot()[0]
        suite.remove(next(c for c in suite if c.get("name") == name))
        suite.set("tests", str(len(suite)))
        tree.getroot().set("tests", str(len(suite)))
        tree.write(p / "junit.xml")
        with self.assertRaisesRegex(ValueError, "union"):
            self.check()

    def test_missing_completed_result(self):
        p = self.root / "native-exam-shard-1/junit.xml"
        tree = exam.ET.parse(p)
        tree.getroot()[0].remove(tree.getroot()[0][0])
        tree.write(p)
        with self.assertRaises(ValueError):
            self.check()

    def test_mismatched_sha(self):
        self.change("metadata.json", lambda m: m.update(source="b" * 40))
        with self.assertRaisesRegex(ValueError, "identity"):
            self.check()

    def test_mismatched_tool(self):
        self.change("metadata.json", lambda m: m["tools"].update(node="v22.0.0"))
        with self.assertRaisesRegex(ValueError, "tools"):
            self.check()

    def test_mismatched_hash(self):
        self.change("metadata.json", lambda m: m["hashes"].update({"Cargo.lock": "e" * 64}))
        with self.assertRaisesRegex(ValueError, "metadata"):
            self.check()

    def test_green_wrapper_failed_child(self):
        self.change("result.json", lambda r: r.update(rc=100))
        with self.assertRaisesRegex(ValueError, "child exit"):
            self.check()

    def test_failed_exit_code_file(self):
        (self.root / "native-exam-shard-1/exit-code.txt").write_text("100\n")
        with self.assertRaisesRegex(ValueError, "actual child"):
            self.check()

    def test_cancellation_and_failure(self):
        for status in ("cancelled", "failure", "skipped", ""):
            with self.subTest(status=status):
                self.jobs["shards"] = status
                with self.assertRaisesRegex(ValueError, "native job"):
                    self.check()

    def test_incomplete_receipt(self):
        self.change("result.json", lambda r: r.update(completed=False))
        with self.assertRaises(ValueError):
            self.check()

    def test_failed_timeout_skipped_and_retry_results(self):
        p = self.root / "native-exam-shard-1/junit.xml"
        original = p.read_text()
        for tag in ("failure", "error", "skipped", "rerunFailure", "flakyFailure"):
            with self.subTest(tag=tag):
                p.write_text(original)
                tree = exam.ET.parse(p)
                exam.ET.SubElement(tree.getroot()[0][0], tag)
                tree.write(p)
                with self.assertRaisesRegex(ValueError, "JUnit result"):
                    self.check()

    def test_doctest_missing_or_failed(self):
        p = self.root / "native-exam-doctests/run.log"
        original = p.read_text()
        for text in (original.replace(" ... ok", " ... FAILED"), "no tests ran"):
            p.write_text(text)
            with self.assertRaises(ValueError):
                self.check()

    def test_full_inventory_disagreement(self):
        self.change("full.json", lambda d: d.update({"test-count": 999}))
        with self.assertRaises(ValueError):
            self.check()

    def test_wrong_attempt(self):
        self.change("metadata.json", lambda m: m.update(attempt="2"))
        with self.assertRaises(ValueError):
            self.check()

    def test_missing_tool_metadata(self):
        self.change("metadata.json", lambda m: m["tools"].pop("pnpm"))
        with self.assertRaisesRegex(ValueError, "tool metadata"):
            self.check()

    def test_missing_platform_metadata(self):
        self.change("metadata.json", lambda m: m.pop("image_version"))
        with self.assertRaisesRegex(ValueError, "metadata"):
            self.check()

    def test_duplicate_json_key(self):
        p = self.root / "duplicate.json"
        p.write_text('{"test-count": 1, "test-count": 2}')
        with self.assertRaisesRegex(ValueError, "duplicate JSON"):
            exam.read_json(p)


if __name__ == "__main__":
    unittest.main()
