#!/usr/bin/env python3
"""Offline refusal controls for qualify-target.py; no native qualification runs here."""
import importlib.util
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("qualify-target.py")
spec = importlib.util.spec_from_file_location("qualify_target", SCRIPT)
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)
NATIVE = qualify.host_identity()
NATIVE_TARGET = next((t for t, h in qualify.NATIVE_HOSTS.items() if h == (NATIVE["system"], NATIVE["machine"])), None)


def host(system, machine, translated=None, cpuinfo=None):
    return {"system": system, "machine": machine, "rosetta_translated": translated, "cpuinfo_head": cpuinfo}


def elf(machine):
    header = bytearray(64)
    header[:4] = b"\x7fELF"
    header[4], header[5] = 2, 1
    header[18:20] = struct.pack("<H", machine)
    return bytes(header)


def macho(cputype):
    return b"\xcf\xfa\xed\xfe" + struct.pack("<I", cputype) + bytes(24)


def pe(machine):
    header = bytearray(256)
    header[:2] = b"MZ"
    header[60:64] = struct.pack("<I", 128)
    header[128:132] = b"PE\0\0"
    header[132:134] = struct.pack("<H", machine)
    return bytes(header)


class NativeRefusal(unittest.TestCase):
    def test_every_target_accepts_only_its_native_host(self):
        for target, (system, machine) in qualify.NATIVE_HOSTS.items():
            self.assertIsNone(qualify.native_refusal(target, host(system, machine)), target)
            for other, other_host in qualify.NATIVE_HOSTS.items():
                if other_host != (system, machine):
                    self.assertIsNotNone(qualify.native_refusal(target, host(*other_host)), (target, other))

    def test_rosetta_is_not_native_intel(self):
        self.assertIn("Rosetta", qualify.native_refusal("x86_64-apple-darwin", host("Darwin", "x86_64", True)))

    def test_qemu_user_arm64_on_x86_cpu_is_refused(self):
        info = "processor\t: 0\nvendor_id\t: GenuineIntel\n"
        self.assertIn("QEMU", qualify.native_refusal("aarch64-unknown-linux-gnu", host("Linux", "arm64", cpuinfo=info)))
        self.assertIsNone(qualify.native_refusal("aarch64-unknown-linux-gnu",
                                                 host("Linux", "arm64", cpuinfo="CPU implementer\t: 0x41\n")))

    def test_targets_match_graph_shipping_set(self):
        graph_spec = importlib.util.spec_from_file_location("record_graph", SCRIPT.with_name("record-graph.py"))
        graph = importlib.util.module_from_spec(graph_spec)
        graph_spec.loader.exec_module(graph)
        self.assertEqual(sorted(qualify.NATIVE_HOSTS), sorted(graph.SHIPPING_TARGETS))
        self.assertEqual(sorted(qualify.BINARY_FORMATS), sorted(graph.SHIPPING_TARGETS))


class DefaultBinary(unittest.TestCase):
    def test_windows_build_output_has_exe_suffix(self):
        self.assertEqual(qualify.default_binary("x86_64-pc-windows-msvc").name, "zfb.exe")
        for target in qualify.NATIVE_HOSTS:
            if "windows" not in target:
                self.assertEqual(qualify.default_binary(target).name, "zfb", target)


class DynamicDependencies(unittest.TestCase):
    def test_objdump_fallback_keeps_only_import_dll_names(self):
        dump = "zfb.exe:     file format pei-x86-64\n\tDLL Name: KERNEL32.dll\n\tvma:  Hint/Ord\n\tDLL Name: ws2_32.dll\n"
        which, run = qualify.shutil.which, qualify.run
        qualify.shutil.which = lambda name: name if name == "objdump" else None
        qualify.run = lambda command: {"command": command, "exit": 0, "stdout": dump, "stderr": ""}
        try:
            result = qualify.dynamic_dependencies("zfb.exe", "Windows")
        finally:
            qualify.shutil.which, qualify.run = which, run
        self.assertEqual(result["stdout"], "DLL Name: KERNEL32.dll\nDLL Name: ws2_32.dll\n")
        self.assertTrue(result["passed"])


class BinaryHeader(unittest.TestCase):
    def test_each_target_header_is_recognized(self):
        samples = {"x86_64-unknown-linux-gnu": elf(62), "aarch64-unknown-linux-gnu": elf(183),
                   "aarch64-apple-darwin": macho(0x0100000C), "x86_64-apple-darwin": macho(0x01000007),
                   "x86_64-pc-windows-msvc": pe(0x8664)}
        for target, data in samples.items():
            self.assertEqual(qualify.binary_architecture(data), qualify.BINARY_FORMATS[target], target)

    def test_wrong_corrupt_and_universal_headers_never_match(self):
        for data in (elf(40), macho(7), pe(0x14C), b"#!/bin/sh\n", b"\x7fELF", b"", b"\xca\xfe\xba\xbe" + bytes(8)):
            self.assertNotIn(qualify.binary_architecture(data), qualify.BINARY_FORMATS.values(), data[:8])


class Verdict(unittest.TestCase):
    def test_unperformed_checks_make_partial_never_pass(self):
        self.assertEqual(qualify.status_of({"a": {"passed": True}, "b": {"passed": None}}), "PARTIAL")
        self.assertEqual(qualify.status_of({"a": {"passed": True}, "b": {"status": "NOT RUN"}}), "PARTIAL")
        self.assertEqual(qualify.status_of({"a": {"passed": False}, "b": {"passed": None}}), "FAIL")
        self.assertEqual(qualify.status_of({"a": {"passed": True}}), "PASS")


class Command(unittest.TestCase):
    def run_script(self, tmp, *argv):
        return subprocess.run([sys.executable, str(SCRIPT), *argv], capture_output=True, text=True, cwd=tmp)

    def fake(self, tmp, name):
        marker = Path(tmp) / f"{name}.executed"
        path = Path(tmp) / name
        path.write_text(f"#!/bin/sh\ntouch {marker}\n")
        path.chmod(0o755)
        return path, marker

    def test_foreign_target_writes_not_run_without_executing(self):
        foreign = next(t for t, h in qualify.NATIVE_HOSTS.items() if h != (NATIVE["system"], NATIVE["machine"]))
        with tempfile.TemporaryDirectory() as tmp:
            binary, marker = self.fake(tmp, "zfb")
            result = self.run_script(tmp, "--target", foreign, "--binary", binary, "--source-sha", "0" * 40,
                                     "--output", Path(tmp) / "row")
            self.assertEqual(result.returncode, 3, result.stderr)
            self.assertEqual(json.loads((Path(tmp) / "row/row.json").read_text())["status"], "NOT RUN")
            self.assertFalse(marker.exists())

    @unittest.skipIf(NATIVE_TARGET is None or sys.platform == "win32", "needs a POSIX shipping host")
    def test_non_executable_header_on_native_target_is_refused_unexecuted(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, marker = self.fake(tmp, "zfb")
            result = self.run_script(tmp, "--target", NATIVE_TARGET, "--binary", binary, "--source-sha", "0" * 40,
                                     "--output", Path(tmp) / "row")
            self.assertEqual(result.returncode, 3, result.stderr)
            row = json.loads((Path(tmp) / "row/row.json").read_text())
            self.assertEqual(row["status"], "NOT RUN")
            self.assertIn("binary header", row["reason"])
            self.assertFalse(marker.exists())

    @unittest.skipIf(NATIVE_TARGET is None or sys.platform == "win32", "needs a POSIX shipping host")
    def test_wrong_source_sha_refuses_to_build(self):
        with tempfile.TemporaryDirectory() as tmp:
            cargo, marker = self.fake(tmp, "cargo")
            result = self.run_script(tmp, "--target", NATIVE_TARGET, "--build", "--cargo", cargo,
                                     "--source-sha", "0" * 40, "--output", Path(tmp) / "row")
            self.assertEqual(result.returncode, 1, result.stderr)
            row = json.loads((Path(tmp) / "row/row.json").read_text())
            self.assertFalse(row["checks"]["source"]["passed"])
            self.assertEqual(row["checks"]["build"]["status"], "NOT RUN")
            self.assertNotIn("command", row["checks"]["build"])
            self.assertFalse((Path(tmp) / "row/build.log").exists())

    def test_existing_output_is_a_usage_refusal(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_script(tmp, "--target", "x86_64-unknown-linux-gnu", "--binary", "missing",
                                     "--source-sha", "0" * 40, "--output", tmp)
            self.assertEqual(result.returncode, 2)
            self.assertIn("already exists", result.stderr)
            self.assertFalse((Path(tmp) / "row.json").exists())

    def test_binary_or_build_is_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_script(tmp, "--target", "x86_64-unknown-linux-gnu", "--source-sha", "0" * 40,
                                     "--output", Path(tmp) / "row")
            self.assertEqual(result.returncode, 2)
            self.assertFalse((Path(tmp) / "row").exists())


if __name__ == "__main__":
    unittest.main()
