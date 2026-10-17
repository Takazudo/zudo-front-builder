#!/usr/bin/env python3
"""External 30s POSIX safety supervisor for the isolated drain tests."""
from pathlib import Path
import os
import signal
import subprocess
import sys

child = subprocess.Popen([sys.executable, '-I', '-B', '-W', 'error::ResourceWarning',
                          '-m', 'unittest', 'discover', '-s', str(Path(__file__).resolve().parent),
                          '-p', 'test_watchdog_drain_prototype.py'], start_new_session=True)
try:
    code = child.wait(timeout=30)
except BaseException:
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=5)
    raise
raise SystemExit(code)
