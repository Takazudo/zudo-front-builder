#!/usr/bin/env python3
"""Offline publication fault injection; not native ICU/runtime qualification."""
import concurrent.futures
import contextlib
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import threading
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("prepare_icu", Path(__file__).with_name("prepare-icu.py"))
icu = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icu)
DATA = b"synthetic verified ICU data\0" * 8
MEMBER = "deno_core_icudata-0.77.0/src/icudtl.dat"


class PartialWriter:
    def __init__(self, stream):
        self.stream = stream

    def __getattr__(self, name):
        return getattr(self.stream, name)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return self.stream.__exit__(*args)

    def write(self, data):
        self.stream.write(data[:7])
        raise OSError("injected partial write")


class Preparation(unittest.TestCase):
    def setUp(self):
        self.room = tempfile.TemporaryDirectory()
        self.addCleanup(self.room.cleanup)
        self.root = Path(self.room.name)
        self.destination = self.root / "target/direct-v8-inputs/icu77/icudtl.dat"
        self.destination.parent.mkdir(parents=True)
        self.archive = self.root / "icu.crate"
        with tarfile.open(self.archive, "w:gz") as archive:
            member = tarfile.TarInfo(MEMBER)
            member.size = len(DATA)
            archive.addfile(member, io.BytesIO(DATA))
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        for name, value in (("__file__", str(self.root / "prototypes/direct-v8/prepare-icu.py")),
                            ("ARCHIVE_SHA256", hashlib.sha256(self.archive.read_bytes()).hexdigest()),
                            ("DATA_SHA256", hashlib.sha256(DATA).hexdigest())):
            self.stack.enter_context(mock.patch.object(icu, name, value))
        self.stack.enter_context(mock.patch("sys.argv", ["prepare-icu.py", "--archive", str(self.archive)]))
        self.stack.enter_context(mock.patch.object(icu.urllib.request, "urlopen", side_effect=AssertionError("offline test used network")))

    def run_prepare(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            icu.main()
        self.assertEqual(output.getvalue().strip(), str(self.destination))

    def test_success_preserves_exact_content_and_path(self):
        self.run_prepare()
        self.assertEqual(self.destination.read_bytes(), DATA)

    def inject_partial_write(self):
        original = io.open

        def open_fault(*args, **kwargs):
            stream = original(*args, **kwargs)
            mode = kwargs.get("mode", args[1] if len(args) > 1 else "r")
            return PartialWriter(stream) if "w" in mode else stream

        return mock.patch.object(io, "open", side_effect=open_fault)

    def test_partial_write_preserves_existing_good_destination(self):
        self.destination.write_bytes(DATA)
        with self.inject_partial_write(), self.assertRaisesRegex(OSError, "injected partial write"):
            icu.main()
        self.assertEqual(self.destination.read_bytes(), DATA)
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_partial_write_does_not_publish_new_destination(self):
        with self.inject_partial_write(), self.assertRaisesRegex(OSError, "injected partial write"):
            icu.main()
        self.assertFalse(self.destination.exists())
        self.assertEqual(list(self.destination.parent.iterdir()), [])

    def assert_preserved(self):
        self.assertEqual(self.destination.read_bytes(), DATA)
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_archive_hash_failure_never_stages(self):
        self.destination.write_bytes(DATA)
        with mock.patch.object(icu, "ARCHIVE_SHA256", "0" * 64), self.assertRaisesRegex(SystemExit, "archive checksum"):
            icu.main()
        self.assert_preserved()

    def test_data_hash_failure_never_stages(self):
        self.destination.write_bytes(DATA)
        with mock.patch.object(icu, "DATA_SHA256", "0" * 64), self.assertRaisesRegex(SystemExit, "ICU data differs"):
            icu.main()
        self.assert_preserved()

    def test_missing_member_never_stages(self):
        self.destination.write_bytes(DATA)
        with tarfile.open(self.archive, "w:gz"):
            pass
        with mock.patch.object(icu, "ARCHIVE_SHA256", hashlib.sha256(self.archive.read_bytes()).hexdigest()):
            with self.assertRaises(KeyError):
                icu.main()
        self.assert_preserved()

    def test_replace_failure_preserves_existing_and_cleans_sibling(self):
        self.destination.write_bytes(DATA)
        with mock.patch.object(icu.os, "replace", side_effect=OSError("injected replace failure")):
            with self.assertRaisesRegex(OSError, "injected replace failure"):
                icu.main()
        self.assert_preserved()

    def test_staged_corruption_is_rejected_before_replace(self):
        self.destination.write_bytes(DATA)
        original = Path.read_bytes

        def corrupt_read(path):
            return b"corrupted staged bytes" if path.name.startswith(".icudtl-") else original(path)

        with mock.patch.object(Path, "read_bytes", corrupt_read), mock.patch.object(icu.os, "replace") as replace:
            with self.assertRaisesRegex(ValueError, "staged ICU data differs"):
                icu.main()
            replace.assert_not_called()
        self.assert_preserved()

    def test_failed_first_replace_does_not_publish(self):
        with mock.patch.object(icu.os, "replace", side_effect=OSError("replace failed")):
            with self.assertRaises(OSError):
                icu.main()
        self.assertEqual(list(self.destination.parent.iterdir()), [])

    def test_directory_destination_is_not_removed(self):
        self.destination.mkdir()
        marker = self.destination / "keep"
        marker.write_bytes(b"keep")
        with self.assertRaises(OSError):
            icu.main()
        self.assertEqual(marker.read_bytes(), b"keep")
        self.assertEqual(list(self.destination.parent.iterdir()), [self.destination])

    def test_destination_symlink_is_replaced_without_writing_target(self):
        target = self.root / "unrelated"
        target.write_bytes(b"untouched")
        try:
            self.destination.symlink_to(target)
        except OSError as error:
            self.skipTest(f"symlink creation unavailable: {error}")
        self.run_prepare()
        self.assertFalse(self.destination.is_symlink())
        self.assertEqual(self.destination.read_bytes(), DATA)
        self.assertEqual(target.read_bytes(), b"untouched")

    def test_concurrent_preparers_use_distinct_complete_siblings(self):
        self.destination.write_bytes(DATA)
        barrier = threading.Barrier(2, timeout=5)
        original = icu.os.replace
        staged = []

        def replace(source, destination):
            self.assertEqual(source.read_bytes(), DATA)
            self.assertEqual(destination.read_bytes(), DATA)
            staged.append(source)
            barrier.wait()
            original(source, destination)

        with mock.patch.object(icu.os, "replace", side_effect=replace):
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(icu.publish_verified_data, self.destination, DATA) for _ in range(2)]
                for future in futures:
                    future.result(timeout=10)
        self.assertEqual(len(set(staged)), 2)
        self.assert_preserved()

    def test_cleanup_failure_preserves_original_exception_and_reports_cause(self):
        self.destination.write_bytes(DATA)
        with mock.patch.object(icu.os, "replace", side_effect=OSError("publication failed")), mock.patch.object(Path, "unlink", side_effect=OSError("cleanup failed")):
            with self.assertRaisesRegex(OSError, "publication failed") as raised:
                icu.main()
        self.assertEqual(str(raised.exception.__cause__), "cleanup failed")
        self.assertEqual(self.destination.read_bytes(), DATA)
        # A refused unlink necessarily leaves a uniquely named sibling, never a partial final file.
        self.assertEqual(len(list(self.destination.parent.glob(".icudtl-*"))), 1)

    def test_interruption_cleans_staging_and_preserves_destination(self):
        self.destination.write_bytes(DATA)
        with mock.patch.object(icu.os, "replace", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                icu.main()
        self.assert_preserved()


if __name__ == "__main__":
    unittest.main()
