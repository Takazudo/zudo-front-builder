#!/usr/bin/env python3
"""Offline tests for prepare-fixtures.py's esbuild-free native producer helpers (#4635)."""
import contextlib
import hashlib
import importlib.util
import io
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("prepare_fixtures", HERE / "prepare-fixtures.py")
prepare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(prepare)


def struct_fields(source, name):
    body = re.search(r"pub struct " + name + r" \{(.*?)\n\}", source, re.S).group(1)
    return re.findall(r"^\s*pub (\w+):", body, re.M)


class BundlerIdentityTest(unittest.TestCase):
    def test_expected_bundler_reads_the_checkout_backend_identity(self):
        source = (prepare.ROOT / prepare.BACKEND_SOURCE).read_text()
        self.assertIn(f'pub const BACKEND_IDENTITY: &str = "{prepare.expected_bundler()}";', source)

    def test_embedded_bundler_needs_exactly_one_report_line(self):
        self.assertEqual(prepare.embedded_bundler("5.2.2\nembedded bundler: rolldown-24bc2d0"),
                         "rolldown-24bc2d0")
        for report in ("5.2.2", "5.2.2\nembedded bundler: a\nembedded bundler: b",
                       "5.2.2\n embedded bundler: rolldown-24bc2d0"):
            with self.subTest(report=report), self.assertRaises(RuntimeError):
                prepare.embedded_bundler(report)


class BundleRequestTest(unittest.TestCase):
    def test_request_carries_every_bundle_request_field_and_no_other(self):
        source = (prepare.ROOT / prepare.BACKEND_SOURCE).read_text()
        request = prepare.bundle_request("/c", "/c/e.ts", "/c/o.mjs", target="es2022")
        self.assertEqual(list(request), struct_fields(source, "BundleRequest"))
        self.assertEqual(list(request["resolve"]), struct_fields(source, "Resolve"))
        self.assertEqual(list(request["jsx"]), struct_fields(source, "Jsx"))

    def test_request_spells_the_former_esbuild_flags_as_fields(self):
        request = prepare.bundle_request(
            "/c", "/c/e.ts", "/c/o.mjs", target="es2022", aliases=[("zfb/config", "/s.mjs")],
            externals=["plugin:answer"], source_map="Linked")
        self.assertEqual(request["output"], {"File": {"file": "/c/o.mjs", "asset_names": None}})
        self.assertEqual((request["format"], request["platform"], request["target"]),
                         ("Esm", "Neutral", "es2022"))
        self.assertEqual(request["resolve"]["aliases"], [["zfb/config", "/s.mjs"]])
        self.assertEqual(request["externals"], [{"Package": "plugin:answer"}])
        self.assertEqual(request["source_map"], "Linked")
        self.assertFalse(request["splitting"] or request["minify"])


class PublishWorkerFilesTest(unittest.TestCase):
    def setUp(self):
        holder = tempfile.TemporaryDirectory()
        self.addCleanup(holder.cleanup)
        self.tmp = Path(holder.name)
        self.private = self.tmp / "private"
        (self.private / "files").mkdir(parents=True)
        self.out = self.tmp / "out"
        self.out.mkdir()
        self.files = []
        for index, (name, data) in enumerate([("a.mjs", b"export {};\n"), ("a.mjs.map", b"{}")]):
            (self.private / "files" / str(index)).write_bytes(data)
            self.files.append({"destination": str(self.out / name), "private": f"files/{index}",
                               "size": len(data), "sha256": hashlib.sha256(data).hexdigest()})

    def response(self, **outcome):
        return {"protocol": 1, "job_id": "job", "nonce": "n",
                "outcome": outcome or {"Generated": {"files": self.files, "graph": {}, "warnings": ["w"]}}}

    def publish(self, response, expected=("a.mjs", "a.mjs.map")):
        return prepare.publish_worker_files(response, "job", "n", self.private,
                                            [self.out / name for name in expected])

    def test_copies_exactly_the_reported_files(self):
        self.assertEqual(self.publish(self.response()), ["w"])
        self.assertEqual((self.out / "a.mjs").read_bytes(), b"export {};\n")
        self.assertEqual((self.out / "a.mjs.map").read_bytes(), b"{}")

    def test_refuses_another_job_identity(self):
        for key, value in (("protocol", 2), ("job_id", "other"), ("nonce", "forged")):
            response = self.response() | {key: value}
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, "job identity"):
                self.publish(response)

    def test_refuses_a_compile_error(self):
        with self.assertRaisesRegex(RuntimeError, "did not generate"):
            self.publish(self.response(CompileError={"diagnostic": "boom"}))

    def test_refuses_a_misreported_digest_or_size(self):
        for key, value in (("sha256", "0" * 64), ("size", 999)):
            self.files[0] = self.files[0] | {key: value}
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, "misreported"):
                self.publish(self.response())
            self.setUp()

    def test_refuses_a_file_outside_the_private_directory(self):
        (self.tmp / "escaped").write_bytes(b"export {};\n")
        self.files[0]["private"] = "../escaped"
        with self.assertRaisesRegex(RuntimeError, "outside its private directory"):
            self.publish(self.response())

    def test_refuses_a_different_output_set(self):
        with self.assertRaisesRegex(RuntimeError, "expected"):
            self.publish(self.response(), expected=("a.mjs",))
        with self.assertRaisesRegex(RuntimeError, "expected"):
            self.publish(self.response(), expected=("a.mjs", "a.mjs.map", "b.mjs"))

    def test_refuses_an_unreported_private_file(self):
        (self.private / "files" / "unreported").write_bytes(b"x")
        with self.assertRaisesRegex(RuntimeError, "unreported"):
            self.publish(self.response())

    def test_never_overwrites_an_existing_output(self):
        (self.out / "a.mjs.map").write_bytes(b"old")
        with self.assertRaisesRegex(RuntimeError, "overwrite"):
            self.publish(self.response())
        self.assertEqual((self.out / "a.mjs.map").read_bytes(), b"old")


class CommandLineTest(unittest.TestCase):
    def test_esbuild_is_no_longer_an_accepted_tool(self):
        argv = ["prepare-fixtures.py", "--zfb", "z", "--esbuild", "e", "--wrapper-test-binary", "w",
                "--rustc", "r", "--node", "n", "--producer-build-command", "c", "--output", "o"]
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(io.StringIO()) as err, \
                self.assertRaises(SystemExit):
            prepare.main()
        self.assertIn("unrecognized arguments: --esbuild", err.getvalue())


if __name__ == "__main__":
    unittest.main()
