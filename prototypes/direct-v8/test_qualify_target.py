#!/usr/bin/env python3
"""Offline refusal controls for qualify-target.py; no native qualification runs here."""
import importlib.util
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("qualify-target.py")
spec = importlib.util.spec_from_file_location("qualify_target", SCRIPT)
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)


def host(system, machine, translated=None):
    return {"system": system, "machine": machine, "rosetta_translated": translated}


class NativeRefusal(unittest.TestCase):
    def test_every_target_accepts_only_its_native_host(self):
        for target, (system, machine) in qualify.NATIVE_HOSTS.items():
            self.assertIsNone(qualify.native_refusal(target, host(system, machine)), target)
            for other, other_host in qualify.NATIVE_HOSTS.items():
                if other_host != (system, machine):
                    self.assertIsNotNone(qualify.native_refusal(target, host(*other_host)), (target, other))

    def test_rosetta_is_not_native_intel(self):
        self.assertIn("Rosetta", qualify.native_refusal("x86_64-apple-darwin", host("Darwin", "x86_64", True)))

    def test_targets_match_graph_shipping_set(self):
        graph_spec = importlib.util.spec_from_file_location("record_graph", SCRIPT.with_name("record-graph.py"))
        graph = importlib.util.module_from_spec(graph_spec)
        graph_spec.loader.exec_module(graph)
        self.assertEqual(sorted(qualify.NATIVE_HOSTS), sorted(graph.SHIPPING_TARGETS))


class Command(unittest.TestCase):
    def test_foreign_target_writes_not_run_without_executing(self):
        native = qualify.host_identity()
        foreign = next(t for t, h in qualify.NATIVE_HOSTS.items() if h != (native["system"], native["machine"]))
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / "executed"
            binary = Path(tmp) / "zfb"
            binary.write_text(f"#!/bin/sh\ntouch {marker}\n")
            binary.chmod(0o755)
            result = subprocess.run([sys.executable, str(SCRIPT), "--target", foreign, "--binary", str(binary),
                                     "--source-sha", "0" * 40, "--output", str(Path(tmp) / "row")],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 3, result.stderr)
            row = json.loads((Path(tmp) / "row/row.json").read_text())
            self.assertEqual(row["status"], "NOT RUN")
            self.assertFalse(marker.exists())

    def test_existing_output_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run([sys.executable, str(SCRIPT), "--target", "x86_64-unknown-linux-gnu",
                                     "--binary", "missing", "--source-sha", "0" * 40, "--output", tmp],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((Path(tmp) / "row.json").exists())


if __name__ == "__main__":
    unittest.main()
