"""Real-process controls for the startup scout's completed transient compiler worker.

Root, native and worker are real python processes standing in for the node shim,
zfb and `zfb __zfb-compiler-worker`, with the same argv, cwd, group and parent
shape. Nothing reaps an exited worker until the case ends, so a zombie seen by
the scout is deterministic. Usage: startup-transient-compiler.py [scout.py]
"""
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

LIB = Path(__file__).resolve().parents[1] / "lib"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


scout_path = Path(sys.argv[1]) if len(sys.argv) > 1 else LIB / "linux-fixed-startup.py"
scout = load("scout", scout_path)
custody = load("custody", LIB / "linux-rss-custody.py")
PY = os.path.realpath(sys.executable)

SHIM = """
import os, subprocess, sys, time
time.sleep(float(os.environ["NATIVE_DELAY"]))
subprocess.Popen([sys.executable, "dev.py"], executable=os.path.realpath(sys.executable))
time.sleep(30)
"""
DEV = """
import os, subprocess, sys, time
if os.environ["DEV"] == "exit":
    sys.exit(0)
time.sleep(float(os.environ["WORKER_DELAY"]))
argv = ["/bin/true"] if os.environ["WORKER"] == "foreign" else [sys.executable, "__zfb-compiler-worker"]
subprocess.Popen(argv, executable=argv[0] if argv[0] == "/bin/true" else os.path.realpath(sys.executable),
                 start_new_session=True)
time.sleep(30)
"""
WORKER = """
import os, signal, sys, time
mode = os.environ["WORKER"]
if mode == "live":
    time.sleep(30)
if mode == "exit1":
    sys.exit(1)
if mode == "signal":
    os.kill(os.getpid(), signal.SIGKILL)
"""


def pin(path):
    info = os.stat(path)
    return dict(path=path, device=str(info.st_dev), inode=str(info.st_ino))


def start_ticks(pid):
    return Path(f"/proc/{pid}/stat").read_bytes().rsplit(b")", 1)[1].split()[19].decode()


def run_case(worker="live", native_delay=0.0, worker_delay=0.0, dev="run", settle=0.25):
    with tempfile.TemporaryDirectory() as cwd:
        for name, body in (("shim.py", SHIM), ("dev.py", DEV), ("__zfb-compiler-worker", WORKER)):
            Path(cwd, name).write_text(body)
        env = dict(os.environ, NATIVE_DELAY=str(native_delay), WORKER_DELAY=str(worker_delay),
                   WORKER=worker, DEV=dev)
        shim = os.path.join(cwd, "shim.py")
        root = subprocess.Popen([PY, shim, "dev.py"], executable=PY, cwd=cwd, env=env,
                                start_new_session=True, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            # Let the roles reach the state under test before the scout starts.
            time.sleep(settle)
            config = dict(launch=dict(cwd=cwd, shim=pin(shim)), root=pin(PY), child=pin(PY))
            seed = dict(pid=root.pid, argv=[shim, "dev.py"], startTicks=start_ticks(root.pid),
                        bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip())
            published = []

            def publish(pair, worker_birth, dev_birth, launch):
                if pair is None:
                    return
                published.append(dict(worker=worker_birth, dev=dev_birth,
                                      retainable=custody.retainable_startup_state(pair[0], worker_birth)))

            s = scout.Startup(config, os.getpid(), seed, publish)
            s.run()
            return s, published
        finally:
            kill_cell(cwd)
            root.wait()


def kill_cell(cwd):
    # The worker runs in its own session, so kill every role by its cwd.
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            try:
                if os.readlink(entry / "cwd") == cwd:
                    os.kill(int(entry.name), signal.SIGKILL)
            except OSError:
                continue


def expect(label, ok, detail=""):
    if not ok:
        raise SystemExit(f"FAIL {label} {detail}")
    results.append(label)


results = []
mode = os.environ.get("CASES", "all")

if mode == "r4-replay":
    # r4 slot 01: the compiler worker had already exited (status 0), unreaped,
    # when the scout reached it.
    s, published = run_case(worker="exit0")
    print(json.dumps(dict(status=s.status, error=s.failure, published=published)))
    raise SystemExit(0)

# 1. The r4 shape: a status-0 zombie compiler worker is a completed transient role.
s, published = run_case(worker="exit0")
expect("r4-zombie-accepted", s.status == "retained" and len(published) == 1, (s.status, s.failure))
w = published[0]["worker"]
t = w["completedTransient"]
expect("r4-transient-record", w["state"] == "Z" and t["exitCode"] == 0 and t["role"] == "__zfb-compiler-worker"
       and t["stat"].split(") ", 1)[1].split()[0] == "Z" and t["parent"]["pid"] == published[0]["dev"]["pid"]
       and t["parent"]["startTicks"] == published[0]["dev"]["startTicks"], t)
expect("r4-holder-retains-terminal", published[0]["retainable"] is True)
expect("r4-private-group", w["pgid"] == w["pid"] and w["ppid"] == published[0]["dev"]["pid"])

# 2. Live passing-start shapes keep the unchanged live binding.
for native_delay, worker_delay, settle in ((0, 0, 0.25), (0.03, 0, 0.25), (0, 0.05, 0.25), (0.05, 0.05, 0.3), (0, 0, 0.0)):
    s, published = run_case(worker="live", native_delay=native_delay, worker_delay=worker_delay, settle=settle)
    ok = s.status == "retained" and len(published) == 1
    w = published[0]["worker"] if published else {}
    expect(f"live-{native_delay}-{worker_delay}-{settle}",
           ok and w.get("state") in ("S", "R") and "completedTransient" not in w and published[0]["retainable"],
           (s.status, s.failure, w))

# 3. Refusals.
for label, kwargs, message in (
    ("crashed-worker-exit-1", dict(worker="exit1"), "startup transient compiler exited unsuccessfully"),
    ("killed-worker", dict(worker="signal"), "startup transient compiler exited unsuccessfully"),
    ("foreign-terminal-child", dict(worker="foreign"), "startup role first seen terminal"),
    ("terminal-zfb-native", dict(dev="exit"), "startup role first seen terminal"),
):
    s, published = run_case(**kwargs)
    expect(label, s.status == "failed" and s.failure == message and not published, (s.status, s.failure))

# 4. A terminal root (the shim) refuses: seed a zombie as the root.
with tempfile.TemporaryDirectory() as cwd:
    shim = os.path.join(cwd, "shim.py")
    Path(shim).write_text("")
    root = subprocess.Popen([PY, shim, "dev.py"], executable=PY, cwd=cwd, start_new_session=True,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.3)
    config = dict(launch=dict(cwd=cwd, shim=pin(shim)), root=pin(PY), child=pin(PY))
    seed = dict(pid=root.pid, argv=[shim, "dev.py"], startTicks=start_ticks(root.pid),
                bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip())
    s = scout.Startup(config, os.getpid(), seed, lambda *a: None)
    s.run()
    root.wait()
    expect("terminal-zfb-root", s.status == "failed" and s.failure == "startup role first seen terminal",
           (s.status, s.failure))

# 5. The completed-role predicate itself, on exact stat inputs (no process needed).
probe = scout.Startup(dict(launch=dict(cwd="/"), root={}, child={}), 1, dict(bootId="b"), lambda *a: None)
parent = dict(pid=200, startTicks="1000")
image = dict(path="/opt/zfb/zfb")
argv = ["/opt/zfb/zfb", "__zfb-compiler-worker"]


def stat(pid=300, comm=b"zfb", state=b"Z", ppid=200, pgid=300, ticks=1001, exit_code=0, fields=52):
    values = [state, str(ppid).encode(), str(pgid).encode()] + [b"0"] * (fields - 5)
    values[19] = str(ticks).encode()
    if fields >= 52:
        values[49] = str(exit_code).encode()
    return str(pid).encode() + b" (" + comm + b") " + b" ".join(values) + b"\n"


def completed(raw, pid=300, ppid=200, pgid=300, pin_=image, argv_=argv):
    head, tail = raw.rsplit(b")", 1)
    state, _, _, *rest = tail.split()
    try:
        probe.completed(None, raw, head, state, rest, pid, ppid, pgid, pin_, argv_, parent)
    except ValueError as error:
        return str(error)
    except FileNotFoundError:
        return "reached-pidfd"  # every stat predicate passed; next is the pidfd read
    return "accepted"


expect("predicate-exact", completed(stat()) == "reached-pidfd")
for label, raw, kwargs, message in (
    ("predicate-dead-state", stat(state=b"X"), {}, "startup role first seen terminal"),
    ("predicate-wrong-parent", stat(ppid=201), dict(ppid=201), "startup role first seen terminal"),
    ("predicate-shared-group", stat(pgid=200), dict(pgid=200), "startup role first seen terminal"),
    ("predicate-wrong-comm", stat(comm=b"node"), {}, "startup role first seen terminal"),
    ("predicate-older-than-parent", stat(ticks=999), {}, "startup role first seen terminal"),
    ("predicate-not-compiler-argv", stat(), dict(argv_=["/opt/zfb/zfb", "dev"]), "startup role first seen terminal"),
    ("predicate-exit-unreadable", stat(fields=51), {}, "startup transient exit status unreadable"),
    ("predicate-exit-nonzero", stat(exit_code=256), {}, "startup transient compiler exited unsuccessfully"),
):
    expect(label, completed(raw, **kwargs) == message, completed(raw, **kwargs))

print("PASS " + " ".join(results))
