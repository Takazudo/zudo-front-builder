#!/usr/bin/env python3
"""Actual CLI build/dev replacement observer; a killed process never passes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import urllib.request

WINDOWS = os.name == "nt"
CONSOLE_CTRL_C = Path(__file__).with_name("console-ctrl-c.py")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def page(marker, *, ssr=False):
    return ("export const prerender = false;\n" if ssr else "") + (
        "export default function Page() { return <html><body>" + marker + "</body></html>; }\n")


def spawn(command, *, cwd, stdout, stderr, env=None, interruptible=False):
    if not WINDOWS:
        return subprocess.Popen(command, cwd=cwd, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
    if not interruptible:
        return subprocess.Popen(command, cwd=cwd, env=env, stdout=stdout, stderr=stderr)
    # Windows has no process-group SIGINT; the helper owns a private hidden
    # console and sends the child a real Ctrl-C there (see console-ctrl-c.py).
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    return subprocess.Popen([sys.executable, str(CONSOLE_CTRL_C), *command], cwd=cwd, env=env,
                            stdin=subprocess.PIPE, stdout=stdout, stderr=stderr,
                            creationflags=subprocess.CREATE_NEW_CONSOLE, startupinfo=startup)


def interrupt(child):
    if WINDOWS:
        child.stdin.write(b"ctrl-c\n")
        child.stdin.close()
    else:
        child.send_signal(signal.SIGINT)


def kill_tree(child):
    if WINDOWS:
        subprocess.run(["taskkill", "/PID", str(child.pid), "/T", "/F"], capture_output=True)
    else:
        os.killpg(child.pid, signal.SIGKILL)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"binary": str(binary), "expected_sha256": args.sha256, "rows": [], "passed": False}
    child = None
    try:
        assert digest(binary) == args.sha256, "binary differs before execution"
        for name in ["build", "dev"]:
            root = output / name
            (root / "pages").mkdir(parents=True)
            (root / "zfb.config.ts").write_text("export default { outDir: 'dist' };\n")
            (root / "pages/index.tsx").write_text(page("CLI_OLD", ssr=name == "dev"))
        with (output / "build.stdout").open("wb") as out, (output / "build.stderr").open("wb") as err:
            child = spawn([str(binary), "build"], cwd=output / "build", stdout=out, stderr=err)
            code = child.wait(timeout=120)
        assert code == 0, f"build exit {code}"
        assert "CLI_OLD" in (output / "build/dist/index.html").read_text(), "missing built page marker"
        report["rows"].append({"name": "typed-config-real-build", "passed": True})
        root = output / "dev"
        stdout = output / "dev.stdout"
        stderr = output / "dev.stderr"
        env = {key: value for key, value in os.environ.items() if not key.startswith("ZFB_DEV_")}
        with stdout.open("wb") as out, stderr.open("wb") as err:
            child = spawn([str(binary), "dev", "--port", "0"], cwd=root, env=env, stdout=out, stderr=err,
                          interruptible=True)
        def logs():
            return stdout.read_text(errors="replace") + stderr.read_text(errors="replace")
        def wait_for(predicate, label, timeout=60):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                assert child.poll() is None, f"dev exited during {label}: {child.returncode}"
                result = predicate()
                if result:
                    return result
                time.sleep(0.05)
            raise AssertionError(f"deadline waiting for {label}")
        match = wait_for(lambda: re.search(r"listening on \S+:(\d+)", logs()), "dev listener", 90)
        url = "http://127.0.0.1:" + match[1] + "/"
        def contains(marker):
            try:
                with urllib.request.urlopen(url, timeout=3) as response:
                    return response.status == 200 and marker in response.read().decode()
            except (OSError, TimeoutError):
                return False
        wait_for(lambda: contains("CLI_OLD"), "initial SSR")
        report["rows"].append({"name": "typed-config-real-dev", "passed": True})
        source = root / "pages/index.tsx"
        source.write_text("throw new Error('CLI_REPLACEMENT_REJECTED');\n" + page("CLI_BAD", ssr=True))
        wait_for(lambda: "CLI_REPLACEMENT_REJECTED" in logs(), "failed candidate boot")
        assert contains("CLI_OLD"), "failed replacement lost last good host"
        report["rows"].append({"name": "failed-replacement-keeps-old-host", "passed": True})
        # The shared timer shim is microtask-backed; use finite CPU work on the
        # candidate isolate thread to expose an observable startup interval.
        source.write_text("const end = Date.now() + 3000; while (Date.now() < end) {}\n"
                          + page("CLI_UPDATED", ssr=True))
        samples = []
        report["continuity_samples"] = samples
        started = time.monotonic()
        while time.monotonic() - started < 60:
            # No retries: even a transient outage during candidate startup fails.
            request_started = time.monotonic()
            with urllib.request.urlopen(url, timeout=1) as response:
                body = response.read().decode()
                assert response.status == 200, "request failed during replacement"
            latency = time.monotonic() - request_started
            assert latency < 1, "request stalled during candidate boot"
            old, updated = "CLI_OLD" in body, "CLI_UPDATED" in body
            assert old != updated, "replacement response has missing or mixed markers"
            samples.append({"elapsed_seconds": time.monotonic() - started,
                            "latency_seconds": latency,
                            "marker": "old" if old else "updated"})
            if updated:
                break
            time.sleep(0.05)
        else:
            raise AssertionError("deadline waiting for updated candidate")
        assert sum(row["marker"] == "old" for row in samples) >= 5, "insufficient old-host observations"
        report["rows"].append({"name": "start-before-swap-continuity", "passed": True})
        assert not contains("CLI_OLD"), "stale old module survived replacement"
        report["rows"].append({"name": "same-source-path-recovery", "passed": True})
        report["shutdown_signal"] = "console CTRL_C_EVENT" if WINDOWS else "SIGINT"
        interrupt(child)
        code = child.wait(timeout=30)
        report["shutdown_exit"] = code
        assert code == 0, f"finite dev shutdown exit {code}"
        report["rows"].append({"name": "finite-console-ctrl-c-clean-exit" if WINDOWS else "finite-sigint-clean-exit",
                               "passed": True})
        assert digest(binary) == args.sha256, "binary changed during execution"
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
    finally:
        if child is not None and child.poll() is None:
            report["watchdog_cleanup"] = True
            kill_tree(child)
            child.wait()
        report["observed_sha256"] = digest(binary) if binary.is_file() else None
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
