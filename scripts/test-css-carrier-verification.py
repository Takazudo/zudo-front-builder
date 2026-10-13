#!/usr/bin/env python3
"""Offline permanent checksum, archive safety and packed-native-byte contracts."""
import importlib.util
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("verification", Path(__file__).with_name("css-carrier-verification.py"))
verification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verification)


def archive(path, entries):
    with tarfile.open(path, "w:gz") as output:
        for name, contents, mode in entries:
            entry = tarfile.TarInfo(name)
            entry.size = len(contents)
            entry.mode = mode
            output.addfile(entry, io.BytesIO(contents))


def checksum(directory, name):
    (directory / (name + ".sha256")).write_text(verification.digest((directory / name).read_bytes()) + "  " + name + "\n")


WINDOWS_RECORD = r"""discovery=default VS developer environment; no build recipe override
VCToolsVersion=14.51.36231
VCToolsInstallDir=C:\VS\VC\Tools\MSVC\14.51.36231\
WindowsSdkDir=C:\Program Files (x86)\Windows Kits\10\
WindowsSDKVersion=10.0.26100.0\
compilerPath=C:\VS\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\cl.exe
linkerPath=C:\VS\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\link.exe
Microsoft (R) C/C++ Optimizing Compiler Version 19.51.36231 for x64
compiler_version_probe_exit=2 (no source compilation requested)
Microsoft (R) Incremental Linker Version 14.51.36231.0
"""


class Contracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.room = Path(self.temporary.name)
        self.received = self.room / "received"
        if len(sys.argv) > 1:
            shutil.copytree(sys.argv[1], self.received)
        else:
            self.received.mkdir()
            archive(self.received / "zfb-4.4.0-x86_64-apple-darwin.tar.gz", [("zfb", b"native CLI fixture", 0o755)])
            (self.received / "zfb-css-4.4.0-x86_64-apple-darwin.node").write_bytes(b"native addon fixture")
            for name in list(self.received.iterdir()):
                checksum(self.received, name.name)

    def tearDown(self):
        self.temporary.cleanup()

    def test_windows_capture_requires_every_selected_record_and_version(self):
        records = verification.windows_toolchain(WINDOWS_RECORD.replace("\n", "\r\n"))
        self.assertEqual(records["compilerVersion"], "19.51.36231")
        self.assertEqual(records["linkerVersion"], "14.51.36231.0")
        log = self.room / "windows-toolchain.txt"
        output = self.room / "windows-toolchain.json"
        log.write_text(WINDOWS_RECORD, encoding="utf-8-sig")
        verification.main(["windows-toolchain", str(log), str(output)])
        self.assertEqual(json.loads(output.read_text()), records)
        # The observed silent PowerShell stdin failure emitted OS/Rust only.
        with self.assertRaisesRegex(ValueError, "missing Windows toolchain"):
            verification.windows_toolchain("Microsoft Windows [Version 10.0.26100]\nrustc1.99.0\n")
        log.write_text("Microsoft Windows [Version 10.0.26100]\nrustc1.99.0\n")
        output.unlink()
        with self.assertRaises(ValueError):
            verification.main(["windows-toolchain", str(log), str(output)])
        self.assertFalse(output.exists())
        for line in WINDOWS_RECORD.splitlines():
            if line.startswith("compiler_version_probe_exit="):
                continue
            with self.subTest(missing=line):
                with self.assertRaises(ValueError):
                    verification.windows_toolchain(WINDOWS_RECORD.replace(line + "\n", ""))

    def test_windows_capture_rejects_empty_duplicate_or_wrong_toolset_paths(self):
        for replacement in [
            WINDOWS_RECORD.replace("VCToolsVersion=14.51.36231", "VCToolsVersion="),
            WINDOWS_RECORD + "WindowsSDKVersion=10.0.26100.0\\\n",
            WINDOWS_RECORD.replace("compilerPath=C:\\VS", "compilerPath=C:\\OtherVS"),
            WINDOWS_RECORD.replace("linkerPath=C:\\VS", "linkerPath=relative\\VS"),
        ]:
            with self.assertRaises(ValueError):
                verification.windows_toolchain(replacement)

    def test_windows_only_matrix_selects_existing_row_and_keeps_all_default(self):
        default = verification.carrier_matrix("false")["include"]
        self.assertEqual(len(default), 5)
        self.assertEqual({row["carrier"] for row in default}, set(verification.CARRIERS))
        windows = verification.carrier_matrix("true")["include"]
        self.assertEqual(windows, [row for row in default if row["carrier"] == "win32-x64-msvc"])
        self.assertEqual(windows[0]["host"], "windows-latest")
        self.assertEqual(windows[0]["target"], "x86_64-pc-windows-msvc")
        for invalid in ["", "windows", "True", "1"]:
            with self.assertRaises(ValueError):
                verification.carrier_matrix(invalid)

    def test_archive_and_addon_have_independent_checksums(self):
        cli, addon, hashes = verification.fast_mac_bytes(self.received)
        self.assertTrue(cli)
        self.assertEqual(hashes["addon"], verification.digest(addon))
        addon_path = self.received / "zfb-css-4.4.0-x86_64-apple-darwin.node"
        addon_path.write_bytes(addon + b"corrupt")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            verification.fast_mac_bytes(self.received)

    def test_missing_addon_rejects_before_staging(self):
        (self.received / "zfb-css-4.4.0-x86_64-apple-darwin.node").unlink()
        with self.assertRaises(FileNotFoundError):
            verification.main(["ingest", str(self.received), str(self.room / "unused-package"), str(self.room / "must-not-exist")])
        self.assertFalse((self.room / "must-not-exist").exists())

    def test_missing_checksum_rejects_before_staging(self):
        (self.received / "zfb-css-4.4.0-x86_64-apple-darwin.node.sha256").unlink()
        with self.assertRaises(FileNotFoundError):
            verification.main(["ingest", str(self.received), str(self.room / "unused-package"), str(self.room / "must-not-exist")])
        self.assertFalse((self.room / "must-not-exist").exists())

    def test_corrupt_addon_rejects_before_staging(self):
        with (self.received / "zfb-css-4.4.0-x86_64-apple-darwin.node").open("ab") as output:
            output.write(b"invalid")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            verification.main(["ingest", str(self.received), str(self.room / "unused-package"), str(self.room / "must-not-exist")])
        self.assertFalse((self.room / "must-not-exist").exists())

    def test_archive_requires_sole_regular_executable_root_cli(self):
        name = "zfb-4.4.0-x86_64-apple-darwin.tar.gz"
        for entries in [
            [("zfb", b"CLI", 0o644)],
            [("zfb", b"CLI", 0o755), ("extra", b"unexpected", 0o644)],
            [("../zfb", b"escape", 0o755)],
        ]:
            archive(self.received / name, entries)
            checksum(self.received, name)
            with self.assertRaises(ValueError):
                verification.fast_mac_bytes(self.received)

    def test_corrupt_archive_checksum_is_rejected(self):
        name = "zfb-4.4.0-x86_64-apple-darwin.tar.gz"
        with (self.received / name).open("ab") as output:
            output.write(b"corrupt")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            verification.fast_mac_bytes(self.received)

    def test_windows_paths_and_normalized_aliases_are_rejected_portably(self):
        packed = self.room / "unsafe.tgz"
        for name in ["package/..\\..\\outside", "C:/outside", "package/C:outside", "\\\\server\\share\\outside", "//server/share/outside"]:
            archive(packed, [(name, b"must not escape", 0o644)])
            with self.assertRaisesRegex(ValueError, "unsafe archive path"):
                verification.checked_tar(packed)
        archive(packed, [("package/file", b"first", 0o644), ("package/./file", b"alias", 0o644)])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            verification.checked_tar(packed)

    def test_symlink_entries_are_never_extracted(self):
        packed = self.room / "symlink.tgz"
        with tarfile.open(packed, "w:gz") as output:
            entry = tarfile.TarInfo("package/link")
            entry.type = tarfile.SYMTYPE
            entry.linkname = "../../outside"
            output.addfile(entry)
        with self.assertRaisesRegex(ValueError, "nonregular"):
            verification.checked_tar(packed)

    def test_pack_compares_original_bytes_and_modes(self):
        cli = self.room / "cli"
        addon = self.room / "addon"
        cli.write_bytes(b"CLI")
        addon.write_bytes(b"addon")
        manifest = json.dumps({"name": "@takazudo/zfb-darwin-x64", "version": "4.4.0"}).encode()
        entries = [("package/package.json", manifest, 0o644), ("package/zfb", b"CLI", 0o755), ("package/zfb-css.node", b"addon", 0o644)]
        packed = self.room / "packed.tgz"
        archive(packed, entries)
        actual = verification.verify_pack(packed, "darwin-x64", cli, addon)
        self.assertEqual(actual["modes"], {"zfb": "0o755", "zfb-css.node": "0o644"})
        for replacement in [("package/zfb", b"changed", 0o755), ("package/zfb", b"CLI", 0o644), ("package/zfb-css.node", b"addon", 0o755)]:
            archive(packed, [replacement if entry[0] == replacement[0] else entry for entry in entries])
            with self.assertRaises(ValueError):
                verification.verify_pack(packed, "darwin-x64", cli, addon)

    def test_three_tarballs_stage_public_exports_without_source_aliases(self):
        packs = self.room / "packs"
        packs.mkdir()
        for name in ["zfb", "zfb-slugify", "zfb-darwin-x64"]:
            manifest = {"name": "@takazudo/" + name, "version": "4.4.0"}
            if name == "zfb":
                manifest["exports"] = {"./css": {"default": "./dist/css.js"}}
            entries = [("package/package.json", json.dumps(manifest).encode(), 0o644)]
            if name == "zfb":
                entries.append(("package/dist/css.js", b"export const compileCss = () => {};", 0o644))
            if name == "zfb-darwin-x64":
                entries += [("package/zfb", b"CLI", 0o755), ("package/zfb-css.node", b"addon", 0o644)]
            archive(packs / f"takazudo-{name}-4.4.0.tgz", entries)
        consumer = self.room / "consumer"
        verification.stage_consumer(packs, "darwin-x64", consumer)
        self.assertTrue((consumer / "node_modules/@takazudo/zfb/dist/css.js").is_file())
        self.assertEqual((consumer / "node_modules/@takazudo/zfb-darwin-x64/zfb-css.node").read_bytes(), b"addon")
        self.assertTrue((consumer / "consumer.mjs").is_file())
        self.assertFalse(any(path.is_symlink() for path in consumer.rglob("*")))
        wrong_manifest = {"name": "@takazudo/zfb", "version": "4.4.0", "exports": {"./css": {"default": "./src/css.ts"}}}
        archive(packs / "takazudo-zfb-4.4.0.tgz", [("package/package.json", json.dumps(wrong_manifest).encode(), 0o644)])
        with self.assertRaisesRegex(ValueError, "emitted dist/css.js"):
            verification.stage_consumer(packs, "darwin-x64", self.room / "wrong-consumer")

    def test_successful_ingestion_is_disposable_and_byte_identical(self):
        package = self.room / "package"
        package.mkdir()
        (package / "package.json").write_text('{"name":"@takazudo/zfb-darwin-x64","version":"4.4.0"}')
        destination = self.room / "staged"
        verification.main(["ingest", str(self.received), str(package), str(destination)])
        cli, addon, _ = verification.fast_mac_bytes(self.received)
        self.assertEqual((destination / "zfb").read_bytes(), cli)
        self.assertEqual((destination / "zfb-css.node").read_bytes(), addon)
        self.assertEqual((destination / "zfb").stat().st_mode & 0o777, 0o755)
        self.assertEqual((destination / "zfb-css.node").stat().st_mode & 0o777, 0o644)
        self.assertFalse((package / "zfb").exists())


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]], verbosity=2)
