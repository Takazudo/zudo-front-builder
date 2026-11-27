"""Holder-level startup custody through the real serve() (#4862 review-2, M6c).

The holder module's serve() runs in this process in startup mode against a real
cell: a root shim, zfb and its compiler worker as python processes. Only the scout
module that serve() loads is wrapped, so a worker can end right before serve()'s
own retention publish runs. Nothing in the production code is patched.
Usage: startup-holder-e2e.py   (prints PASS and the case names)
"""
import contextlib
import importlib.util
import io
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from unittest import mock

LIB = Path(__file__).resolve().parents[1] / "lib"
PY = os.path.realpath(sys.executable)
real_spec = importlib.util.spec_from_file_location


def load(name, path):
    spec = real_spec(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


custody = load("custody", LIB / "linux-rss-custody.py")

SHIM = """
import os, subprocess, sys, time
subprocess.Popen([sys.executable, "dev.py"], executable=os.path.realpath(sys.executable))
time.sleep(30)
"""
DEV = """
import os, subprocess, sys, time
child = subprocess.Popen([sys.executable, "__zfb-compiler-worker"],
                         executable=os.path.realpath(sys.executable), start_new_session=True)
if os.environ["REAP"] == "now":
    child.wait()
time.sleep(30)
"""
WORKER = """
import os, sys, time
if os.environ["WORKER"] == "exit0":
    sys.exit(0)
while not os.path.exists("exit"):
    time.sleep(0.002)
sys.exit(0)
"""


def pin(path):
    info = os.stat(path)
    return dict(path=path, device=str(info.st_dev), inode=str(info.st_ino))


def fields(pid):
    return Path(f"/proc/{pid}/stat").read_bytes().rsplit(b")", 1)[1].split()


def children(pid):
    out = set()
    for task in Path(f"/proc/{pid}/task").iterdir():
        with contextlib.suppress(OSError):
            out.update(map(int, (task / "children").read_text().split()))
    return out


def wait_for(cond, timeout=5.0):
    end = time.monotonic() + timeout
    while not cond():
        if time.monotonic() > end:
            raise SystemExit("FAIL timed out waiting for the cell")
        time.sleep(0.005)


def run(worker, reap, end_before_publish=None, complete_after_retention=False):
    """One holder session. end_before_publish: None, "zombie" or "reaped"."""
    with tempfile.TemporaryDirectory() as cwd:
        for name, body in (("shim.py", SHIM), ("dev.py", DEV), ("__zfb-compiler-worker", WORKER)):
            Path(cwd, name).write_text(body)
        shim = os.path.join(cwd, "shim.py")
        env = dict(os.environ, WORKER=worker, REAP=reap)
        root = subprocess.Popen([PY, shim, "dev.py"], executable=PY, cwd=cwd, env=env,
                                start_new_session=True, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            wait_for(lambda: children(root.pid))
            native = next(iter(children(root.pid)))
            if worker == "exit0":
                wait_for(lambda: int(fields(native)[8]) > 0 and not children(native))
            else:
                wait_for(lambda: children(native))
            worker_pid = next(iter(children(native)), None)
            session = secrets.token_hex(16)
            seed = dict(session=session,
                        root=dict(pid=root.pid, ppid=os.getpid(), pgid=root.pid, argv=[shim, "dev.py"],
                                  startTicks=fields(root.pid)[19].decode(),
                                  bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip()),
                        fixed=dict(launch=dict(cwd=cwd, shim=pin(shim)), root=pin(PY), child=pin(PY)))

            def ended():
                Path(cwd, "exit").write_text("")
                if end_before_publish == "reaped":
                    wait_for(lambda: not Path(f"/proc/{worker_pid}").exists())
                else:
                    wait_for(lambda: fields(worker_pid)[0] == b"Z")

            def spec_from_file_location(name, path, *a, **k):
                spec = real_spec(name, path, *a, **k)
                if name != "fixed_startup":
                    return spec
                loader = spec.loader
                real_exec = loader.exec_module

                def exec_module(module):
                    real_exec(module)
                    base = module.Startup

                    class Startup(base):
                        def __init__(self, config, parent, seed, publish, complete=None):
                            fired = []

                            def wrapped(pair, birth, dev, launch):
                                if pair is not None and end_before_publish and not fired:
                                    fired.append(1)
                                    ended()
                                result = publish(pair, birth, dev, launch)
                                if pair is not None and complete_after_retention:
                                    complete(dict(outcome="product-completion"))
                                return result
                            super().__init__(config, parent, seed, wrapped, complete)
                    module.Startup = Startup
                loader.exec_module = exec_module
                return spec

            stdin_r, stdin_w = os.pipe()
            shared, _peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
            out = io.StringIO()
            errors = []

            def serve():
                try:
                    with open(stdin_r, "rb", buffering=0) as raw, \
                            mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BufferedReader(raw))), \
                            mock.patch.object(importlib.util, "spec_from_file_location",
                                              spec_from_file_location), \
                            contextlib.redirect_stdout(out):
                        custody.serve(shared, os.getpid(), session, True)
                except BaseException as error:  # the holder's require() raises ValueError
                    errors.append(repr(error))

            thread = threading.Thread(target=serve, daemon=True)
            thread.start()
            os.write(stdin_w, (json.dumps(seed) + "\n").encode())
            wait_for(lambda: '"startupReport"' in out.getvalue() or errors, timeout=6)
            os.close(stdin_w)
            thread.join(5)
            shared.close()
            _peer.close()
            rows = [json.loads(x) for x in out.getvalue().splitlines() if x.strip()]
            return rows, errors
        finally:
            for pid in [root.pid, *children(root.pid)]:
                with contextlib.suppress(OSError):
                    for kid in children(pid):
                        os.kill(kid, signal.SIGKILL)
                    os.kill(pid, signal.SIGKILL)
            root.wait()
            for entry in Path("/proc").iterdir():
                if entry.name.isdigit():
                    with contextlib.suppress(OSError):
                        if os.readlink(entry / "cwd") == cwd:
                            os.kill(int(entry.name), signal.SIGKILL)


results = []


def expect(label, ok, detail=""):
    if not ok:
        raise SystemExit(f"FAIL {label} {json.dumps(detail, default=str)[:600]}")
    results.append(label)


def one(rows, key):
    found = [r for r in rows if key in r]
    return found[0] if len(found) == 1 else None


for label, worker, reap, end, outcome in (
    ("holder-live-custody", "wait", "no", None, "live"),
    ("holder-settles-zombie-before-retention", "wait", "no", "zombie", "zombie"),
    ("holder-defers-reaped-before-retention", "wait", "now", "reaped", "completion"),
    ("holder-product-completion", "exit0", "now", None, "completion"),
):
    rows, errors = run(worker, reap, end)
    report = one(rows, "startupReport")
    stopped = one(rows, "stopped")
    birth = one(rows, "startupBirth")
    completion = one(rows, "startupCompletion")
    ok = not errors and report and stopped and report["startupReport"]["error"] is None
    if outcome == "live":
        ok = ok and birth and "completedTransient" not in birth["startupBirth"] and not completion \
            and report["startupReport"]["status"] == "retained" and stopped["retains"] == 1
    elif outcome == "zombie":
        t = birth and birth["startupBirth"].get("completedTransient") or {}
        ok = ok and birth and t.get("exitCode") == 0 and birth["startupBirth"]["state"] == "Z" \
            and int(t["observedNs"]) <= int(birth["liveVerifiedNs"]) and not completion \
            and report["startupReport"]["status"] == "retained" and stopped["retains"] == 1
    else:
        c = completion and completion["startupCompletion"] or {}
        ok = ok and not birth and c.get("outcome") == "product-completion" \
            and report["startupReport"]["status"] == "worker-not-observed" and stopped["retains"] == 0 \
            and (end != "reaped" or c["reapedMidBind"][-1]["reason"] == "exited-reaped-mid-verify")
    expect(label, ok, dict(rows=rows, errors=errors))

# A scout that published a retention must never also publish a completion.
rows, errors = run("wait", "no", complete_after_retention=True)
report = one(rows, "startupReport")
expect("holder-refuses-completion-after-retention",
       not errors and report and report["startupReport"]["status"] == "failed"
       and report["startupReport"]["error"] == "startup completion after retention"
       and not one(rows, "startupCompletion"), dict(rows=rows, errors=errors))

print("PASS " + " ".join(results))
