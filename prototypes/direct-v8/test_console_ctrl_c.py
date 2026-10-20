#!/usr/bin/env python3
"""Offline controls for console-ctrl-c.py; the Ctrl-C delivery itself needs a Windows console."""
from pathlib import Path
import subprocess
import sys
import unittest

HELPER = Path(__file__).with_name("console-ctrl-c.py")


def helper(*command, stdin=subprocess.DEVNULL):
    return subprocess.run([sys.executable, HELPER, *command], stdin=stdin, capture_output=True, text=True,
                          timeout=30)


class ConsoleCtrlC(unittest.TestCase):
    def test_child_exit_code_and_output_pass_through_without_a_request(self):
        # stdin stays open: EOF would itself request the Ctrl-C.
        with subprocess.Popen([sys.executable, HELPER, sys.executable, "-c",
                               "import time; print('child'); time.sleep(0.2); raise SystemExit(7)"],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True) as process:
            stdout = process.stdout.read()
            self.assertEqual(process.wait(timeout=30), 7)
        self.assertEqual(stdout, "child\n")

    def test_usage_refusal(self):
        self.assertEqual(helper().returncode, 2)

    @unittest.skipIf(sys.platform == "win32", "delivery is the Windows path under test in CI")
    def test_request_off_windows_kills_the_child_and_fails(self):
        result = helper(sys.executable, "-c", "import time; time.sleep(30)")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("only be delivered on Windows", result.stderr)


if __name__ == "__main__":
    unittest.main()
