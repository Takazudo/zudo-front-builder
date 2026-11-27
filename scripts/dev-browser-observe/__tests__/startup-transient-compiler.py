"""Real-process controls for the startup scout's compiler-worker slot (#4862).

Root, native and worker are real python processes standing in for the node shim,
zfb and `zfb __zfb-compiler-worker`, with the same argv, cwd, group and parent
shape. Each outcome is driven deterministically: a zombie stays unreaped until
the case ends, a reaped worker is waited for before the scout is released, and
sweeps are held between steps where a bracket must be observed.
Usage: startup-transient-compiler.py [scout.py]   (CASES=r4-replay for one case)
"""
import contextlib
import ctypes
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from unittest import mock

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
# REAP=now waits for the worker the way zfb's supervisor does; REAP=no leaves it
# a zombie. GO names a file the test creates before the worker may start.
DEV = """
import os, subprocess, sys, time
if os.environ["DEV"] == "exit":
    sys.exit(0)
if os.environ["DEV"] == "idle":
    time.sleep(30)
time.sleep(float(os.environ["WORKER_DELAY"]))
go = os.environ.get("GO")
while go and not os.path.exists(go):
    time.sleep(0.002)
argv = ["/bin/true"] if os.environ["WORKER"] == "foreign" else [sys.executable, "__zfb-compiler-worker"]
child = subprocess.Popen(argv, executable=argv[0] if argv[0] == "/bin/true" else os.path.realpath(sys.executable),
                         start_new_session=True)
if os.environ["REAP"] == "now":
    child.wait()
time.sleep(30)
"""
WORKER = """
import ctypes, os, signal, sys, threading, time
mode = os.environ["WORKER"]
if mode == "live":
    time.sleep(30)
if mode == "exit1":
    sys.exit(1)
if mode == "signal":
    os.kill(os.getpid(), signal.SIGKILL)
if mode == "exit-on-file":
    while not os.path.exists("exit"):
        time.sleep(0.002)
    sys.exit(0)
if mode == "leader-exit":
    threading.Thread(target=time.sleep, args=(30,), daemon=False).start()
    ctypes.CDLL(None).pthread_exit(None)
"""


def pin(path):
    info = os.stat(path)
    return dict(path=path, device=str(info.st_dev), inode=str(info.st_ino))


def stat_fields(pid):
    return Path(f"/proc/{pid}/stat").read_bytes().rsplit(b")", 1)[1].split()


def start_ticks(pid):
    return stat_fields(pid)[19].decode()


def children_of(pid):
    out = set()
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            out.update(map(int, (task / "children").read_text().split()))
        except OSError:
            continue
    return out


def wait_for(cond, timeout=5.0):
    end = time.monotonic() + timeout
    while not cond():
        if time.monotonic() > end:
            raise SystemExit("FAIL timed out waiting for the case's process state")
        time.sleep(0.005)


def kill_cell(cwd):
    # The worker runs in its own session, so kill every role by its cwd.
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            try:
                if os.readlink(entry / "cwd") == cwd:
                    os.kill(int(entry.name), signal.SIGKILL)
            except OSError:
                continue


class Cell:
    def __init__(self, worker="live", native_delay=0.0, worker_delay=0.0, dev="run", reap="no", go=False):
        self.dir = tempfile.TemporaryDirectory()
        self.cwd = self.dir.name
        for name, body in (("shim.py", SHIM), ("dev.py", DEV), ("__zfb-compiler-worker", WORKER)):
            Path(self.cwd, name).write_text(body)
        self.go = os.path.join(self.cwd, "go") if go else None
        env = dict(os.environ, NATIVE_DELAY=str(native_delay), WORKER_DELAY=str(worker_delay),
                   WORKER=worker, DEV=dev, REAP=reap, **({"GO": self.go} if go else {}))
        self.shim = os.path.join(self.cwd, "shim.py")
        self.root = subprocess.Popen([PY, self.shim, "dev.py"], executable=PY, cwd=self.cwd, env=env,
                                     start_new_session=True, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def native(self):
        kids = children_of(self.root.pid)
        return next(iter(kids)) if kids else None

    def scout(self, **kwargs):
        config = dict(launch=dict(cwd=self.cwd, shim=pin(self.shim)), root=pin(PY), child=pin(PY))
        seed = dict(pid=self.root.pid, argv=[self.shim, "dev.py"], startTicks=start_ticks(self.root.pid),
                    bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip())
        self.published, self.completions = [], []
        self.before_retention = lambda pair, worker_birth: None

        def publish(pair, worker_birth, dev_birth, launch):
            # The holder's own order (linux-rss-custody.py publish): settle, then poll.
            if pair is not None:
                self.before_retention(pair, worker_birth)
                worker_birth = custody.settle_startup_worker(self.s, pair, worker_birth, dev_birth)
                self.published.append(dict(worker=worker_birth, dev=dev_birth,
                                           retainable=custody.retainable_startup_state(pair[0], worker_birth)))

        import inspect
        extra = ([self.completions.append]
                 if "complete" in inspect.signature(scout.Startup).parameters else [])  # f8ab37bfb lacks it
        self.s = scout.Startup(config, os.getpid(), seed, publish, *extra, **kwargs)
        return self.s

    def close(self):
        kill_cell(self.cwd)
        self.root.wait()
        self.dir.cleanup()


def run_case(settle=0.25, until=None, **kwargs):
    cell = Cell(**kwargs)
    try:
        if until:
            wait_for(lambda: until(cell))
        else:
            time.sleep(settle)
        s = cell.scout()
        s.run()
        return s, cell.published, cell.completions
    finally:
        cell.close()


results = []


def expect(label, ok, detail=""):
    if not ok:
        raise SystemExit(f"FAIL {label} {detail}")
    results.append(label)


if os.environ.get("CASES") == "r4-replay":
    # r4 slot 01: the compiler worker had already exited (status 0), unreaped.
    s, published, _ = run_case(worker="exit0")
    print(json.dumps(dict(status=s.status, error=s.failure, published=published)))
    raise SystemExit(0)

# 0. The stat field indices are pinned: field N is index N - 3 after the ")" of comm.
line = b"77 (zfb) " + b" ".join(str(n).encode() for n in range(3, 53))
tail = line.rsplit(b")", 1)[1].split()
expect("pin-cminflt-is-field-11", tail[scout.STAT_CMINFLT] == b"11")
expect("pin-exit-code-is-field-52", tail[scout.STAT_EXIT_CODE] == b"52")
expect("pin-start-ticks-is-field-22", tail[19] == b"22")
# ...and on the real kernel: a fresh fork reads 0 and a reaped child raises it.
reader, writer = os.pipe()
pid = os.fork()
if pid == 0:
    before = int(stat_fields(os.getpid())[scout.STAT_CMINFLT])
    subprocess.run(["/bin/true"])
    after = int(stat_fields(os.getpid())[scout.STAT_CMINFLT])
    os.write(writer, f"{before} {after}".encode())
    os._exit(0)
os.waitpid(pid, 0)
os.close(writer)
before, after = map(int, os.read(reader, 64).split())
expect("kernel-cminflt-fresh-zero-then-raised", before == 0 and after > 0, (before, after))

# 1. Zombie custody, the r4 shape: a status-0 zombie compiler worker.
s, published, completions = run_case(worker="exit0")
expect("zombie-custody", s.status == "retained" and len(published) == 1 and not completions, (s.status, s.failure))
w = published[0]["worker"]
t = w["completedTransient"]
expect("zombie-record", w["state"] == "Z" and t["exitCode"] == 0 and t["role"] == "__zfb-compiler-worker"
       and t["stat"].split(") ", 1)[1].split()[0] == "Z" and t["parent"]["pid"] == published[0]["dev"]["pid"], t)
expect("zombie-holder-retains-terminal", published[0]["retainable"] is True)

# 2. Live custody, five passing-start shapes.
for native_delay, worker_delay, settle in ((0, 0, 0.25), (0.03, 0, 0.25), (0, 0.05, 0.25), (0.05, 0.05, 0.3), (0, 0, 0.0)):
    s, published, completions = run_case(worker="live", native_delay=native_delay, worker_delay=worker_delay,
                                         settle=settle)
    w = published[0]["worker"] if published else {}
    expect(f"live-custody-{native_delay}-{worker_delay}-{settle}",
           s.status == "retained" and w.get("state") in ("S", "R") and "completedTransient" not in w
           and published[0]["retainable"] and not completions, (s.status, s.failure, w))

# 3. Product completion, worker reaped before the scout ever sees zfb: lower bound is zfb's birth.
s, published, completions = run_case(
    worker="exit0", reap="now",
    until=lambda c: c.native() and int(stat_fields(c.native())[scout.STAT_CMINFLT]) > 0)
expect("product-completion-birth-bracket", s.status == "worker-not-observed" and not published
       and len(completions) == 1 and completions[0]["bracket"]["lower"]["kind"] == "native-birth"
       and completions[0]["bracket"]["upper"]["nativeCminflt"] > 0
       and completions[0]["bracket"]["upper"]["children"] == 0, (s.status, s.failure, completions))

# 4. Product completion with an observed 0 -> >0 step between empty-children sweeps. The
# scout is held after a zero sweep while the worker runs and is reaped, then released.
cell = Cell(worker="exit0", reap="now", go=True)
try:
    wait_for(lambda: cell.native() is not None)
    s = cell.scout()
    hold, resume = threading.Event(), threading.Event()

    def held_wait(_seconds):
        if s.trace and s.trace[-1]["nativeCminflt"] == 0 and s.trace[-1]["children"] == 0 and not hold.is_set():
            hold.set()
            resume.wait(5)
        return False

    s.stop.wait = held_wait
    s.thread.start()
    wait_for(hold.is_set)
    native = cell.native()
    Path(cell.go).write_text("")
    wait_for(lambda: int(stat_fields(native)[scout.STAT_CMINFLT]) > 0 and not children_of(native))
    resume.set()
    s.thread.join(5)
    lower = cell.completions[0]["bracket"]["lower"] if cell.completions else {}
    expect("product-completion-observed-zero-bracket",
           s.status == "worker-not-observed" and lower.get("kind") == "observed-zero"
           and lower.get("nativeCminflt") == 0
           and lower["sweep"] < cell.completions[0]["bracket"]["upper"]["sweep"],
           (s.status, s.failure, cell.completions))
finally:
    cell.close()

# 5. Missed: zfb live, no worker and no reaped child before the scan deadline.
cell = Cell(dev="idle")
try:
    wait_for(lambda: cell.native() is not None)
    s = cell.scout()
    s.end_ns = s.start_ns + 300_000_000
    s.run()
    expect("missed-without-reap", s.status == "missed" and not cell.completions and not cell.published,
           (s.status, s.failure))
finally:
    cell.close()

# 6. Refusals.
for label, kwargs, message in (
    ("crashed-worker-exit-1", dict(worker="exit1"), "startup transient compiler exited unsuccessfully"),
    ("killed-worker", dict(worker="signal"), "startup transient compiler exited unsuccessfully"),
    ("foreign-terminal-child", dict(worker="foreign"), "startup role first seen terminal"),
    ("terminal-zfb-native", dict(dev="exit"), "startup role first seen terminal"),
    ("leader-exit-live-thread", dict(worker="leader-exit"), "startup transient pidfd not terminal"),
):
    s, published, completions = run_case(**kwargs)
    expect(label, s.status == "failed" and s.failure == message and not published and not completions,
           (s.status, s.failure))

# 7. A zombie read without ptrace access has exit_code 0 by default: refuse it.
with mock.patch.object(os, "geteuid", return_value=os.getuid() + 1):
    s, published, _ = run_case(worker="exit0")
expect("zombie-foreign-owner", s.status == "failed" and s.failure == "startup transient exit status unreadable"
       and not published, (s.status, s.failure))

# 8. Terminal root (the shim): seed a zombie as the root.
with tempfile.TemporaryDirectory() as cwd:
    shim = os.path.join(cwd, "shim.py")
    Path(shim).write_text("")
    root = subprocess.Popen([PY, shim, "dev.py"], executable=PY, cwd=cwd, start_new_session=True,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.3)
    config = dict(launch=dict(cwd=cwd, shim=pin(shim)), root=pin(PY), child=pin(PY))
    seed = dict(pid=root.pid, argv=[shim, "dev.py"], startTicks=start_ticks(root.pid),
                bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip())
    s = scout.Startup(config, os.getpid(), seed, lambda *a: None, lambda r: None)
    s.run()
    root.wait()
    expect("terminal-zfb-root", s.status == "failed" and s.failure == "startup role first seen terminal",
           (s.status, s.failure))


# 9. Reaped mid-bind: each kernel answer for an already-reaped worker defers to the next
# sweep with attribution; the same answer for zfb itself still refuses.
def is_worker(pid):
    try:
        return b"__zfb-compiler-worker" in Path(f"/proc/{pid}/cmdline").read_bytes()
    except OSError:
        return False


def is_native(pid):
    try:
        return Path(f"/proc/{pid}/cmdline").read_bytes().endswith(b"dev.py\0")
    except OSError:
        return False


def once(predicate, real, fault):
    fired = []

    def call(*args, **kwargs):
        if not fired and predicate(*args):
            fired.append(1)
            raise fault
        return real(*args, **kwargs)
    return call


real_pidfd_open, real_open = os.pidfd_open, os.open
for label, target, patcher in (
    ("reaped-pidfd-open-esrch", "worker",
     lambda: mock.patch.object(os, "pidfd_open", once(lambda pid, *_: is_worker(pid), real_pidfd_open,
                                                     ProcessLookupError()))),
    ("reaped-procdir-enoent", "worker",
     lambda: mock.patch.object(os, "open", once(
         lambda path, *_: isinstance(path, str) and path.startswith("/proc/") and path.count("/") == 2
         and is_worker(int(path.rsplit("/", 1)[1])), real_open, FileNotFoundError()))),
    ("native-pidfd-open-esrch", "native",
     lambda: mock.patch.object(os, "pidfd_open", once(lambda pid, *_: is_native(pid), real_pidfd_open,
                                                     ProcessLookupError("native gone")))),
    ("native-procdir-enoent", "native",
     lambda: mock.patch.object(os, "open", once(
         lambda path, *_: isinstance(path, str) and path.startswith("/proc/") and path.count("/") == 2
         and is_native(int(path.rsplit("/", 1)[1])), real_open, FileNotFoundError("native gone")))),
):
    cell = Cell(worker="live")
    try:
        wait_for(lambda: cell.native() is not None and children_of(cell.native()))
        s = cell.scout()
        with patcher():
            s.run()
        if target == "worker":
            expect(label, s.status == "retained" and s.reaped_mid_bind
                   and s.reaped_mid_bind[0]["reason"] in ("pidfd-open-esrch", "procdir-enoent"),
                   (s.status, s.failure, s.reaped_mid_bind))
        else:
            # The raw kernel answer surfaces: zfb itself is never deferred as reaped.
            expect(label, s.status == "failed" and s.failure == "native gone" and not s.reaped_mid_bind,
                   (s.status, s.failure))
    finally:
        cell.close()

for label, reason in (("reaped-stat-esrch", "stat-esrch"), ("reaped-fdinfo-minus-one", "fdinfo-pid-minus-one")):
    cell = Cell(worker="live")
    try:
        wait_for(lambda: cell.native() is not None and children_of(cell.native()))
        s = cell.scout()
        real_read = s.read
        fired = []

        def faulty(path, directory=None, _real=real_read, _reason=reason):
            data = _real(path, directory)
            if fired:
                return data
            if _reason == "stat-esrch" and path == "stat" and directory is not None:
                target = int(os.readlink(f"/proc/self/fd/{directory}").rsplit("/", 1)[1])
                if is_worker(target):
                    fired.append(1)
                    raise ProcessLookupError()
            if _reason == "fdinfo-pid-minus-one" and path.startswith("/proc/self/fdinfo/"):
                pid_line = [x for x in data.splitlines() if x.startswith(b"Pid:\t")]
                if pid_line and is_worker(int(pid_line[0].split(b"\t")[1])):
                    fired.append(1)
                    return data.replace(pid_line[0], b"Pid:\t-1")
            return data

        s.read = faulty
        s.run()
        expect(label, s.status == "retained" and s.reaped_mid_bind and s.reaped_mid_bind[0]["reason"] == reason,
               (s.status, s.failure, s.reaped_mid_bind))
    finally:
        cell.close()

cell = Cell(worker="live")
try:
    wait_for(lambda: cell.native() is not None and children_of(cell.native()))
    s = cell.scout()
    real_read = s.read

    def native_stat_gone(path, directory=None):
        if path == "stat" and directory is not None:
            if is_native(int(os.readlink(f"/proc/self/fd/{directory}").rsplit("/", 1)[1])):
                raise ProcessLookupError("native gone")
        return real_read(path, directory)

    s.read = native_stat_gone
    s.run()
    expect("native-stat-esrch", s.status == "failed" and s.failure == "native gone" and not s.reaped_mid_bind,
           (s.status, s.failure))
finally:
    cell.close()


# 9b. review-2 finding 3: a live-listed worker that exits during verification is
# routed by its pidfd and stat: a clean zombie is zombie custody, a reaped worker
# defers and zfb's own reap concludes product completion.
def fd_pid(fd):
    return int(os.readlink(f"/proc/self/fd/{fd}").rsplit("/", 1)[1])


def end_worker(cell, pid, reaped):
    Path(cell.cwd, "exit").write_text("")
    if reaped:
        wait_for(lambda: not Path(f"/proc/{pid}").exists())
    else:
        wait_for(lambda: stat_fields(pid)[0] == b"Z")


for point in ("cmdline", "cwd", "exe", "reverify", "retention"):
    for reaped in (False, True):
        cell = Cell(worker="exit-on-file", reap="now" if reaped else "no")
        try:
            wait_for(lambda: cell.native() is not None and children_of(cell.native()))
            worker_pid = next(iter(children_of(cell.native())))
            s = cell.scout()
            fired = []

            def trigger(pid):
                if pid == worker_pid and not fired:
                    fired.append(point)
                    end_worker(cell, worker_pid, reaped)

            patches = []
            if point == "cmdline":
                real_read = s.read

                def read(path, directory=None, _real=real_read):
                    if path == "cmdline" and directory is not None:
                        trigger(fd_pid(directory))
                    return _real(path, directory)
                s.read = read
            elif point in ("cwd", "exe"):
                name = point
                real = os.readlink if name == "cwd" else os.open

                def hooked(path, *args, _real=real, _name=name, **kwargs):
                    if path == _name and kwargs.get("dir_fd") is not None:
                        trigger(fd_pid(kwargs["dir_fd"]))
                    return _real(path, *args, **kwargs)
                patches.append(mock.patch.object(os, "readlink" if name == "cwd" else "open", hooked))
            elif point == "reverify":
                real_verify = s.verify
                calls = []

                def verify(*args, _real=real_verify):
                    if args[1] == worker_pid:
                        calls.append(1)
                        if len(calls) == 2:
                            trigger(worker_pid)
                    return _real(*args)
                s.verify = verify
            else:
                cell.before_retention = lambda pair, birth: trigger(birth["pid"])
            with contextlib.ExitStack() as stack:
                for p in patches:
                    stack.enter_context(p)
                s.run()
            label = f"mid-verify-exit-{point}-{'reaped' if reaped else 'zombie'}"
            if reaped:
                expect(label, fired and s.status == "worker-not-observed" and not cell.published
                       and len(cell.completions) == 1 and s.reaped_mid_bind
                       # A re-verify starts with stat, which already maps ESRCH.
                       and s.reaped_mid_bind[-1]["reason"] in (
                           ("exited-reaped-mid-verify", "stat-esrch") if point == "reverify"
                           else ("exited-reaped-mid-verify",)),
                       (s.status, s.failure, s.reaped_mid_bind, fired))
            else:
                w = cell.published[0]["worker"] if cell.published else {}
                expect(label, fired and s.status == "retained" and len(cell.published) == 1
                       and w.get("completedTransient", {}).get("exitCode") == 0
                       and cell.published[0]["retainable"] is True and not cell.completions,
                       (s.status, s.failure, fired, w))
        finally:
            cell.close()

# 9c. conclude() re-checks zfb after the decision sweep (review-2 J8, J8b), and a
# native fdinfo -1 is never a deferral (J15).
for label, fault in (("conclude-child-appears", "children"), ("conclude-cminflt-drops", "cminflt")):
    cell = Cell(worker="exit0", reap="now")
    try:
        wait_for(lambda: cell.native() and int(stat_fields(cell.native())[scout.STAT_CMINFLT]) > 0
                 and not children_of(cell.native()))
        s = cell.scout()
        state = dict(inside=False)
        real_conclude, real_children, real_verify = s.conclude, s.children, s.verify

        def conclude(*args):
            state["inside"] = True
            return real_conclude(*args)

        def children(directory):
            found = real_children(directory)
            return found | {999999} if state["inside"] and fault == "children" else found

        def verify(*args):
            out = real_verify(*args)
            if state["inside"] and fault == "cminflt" and args[1] == cell.native():
                out = dict(out, cminflt=0)
            return out
        s.conclude, s.children, s.verify = conclude, children, verify
        s.run()
        expect(label, s.status == "failed" and s.failure == "startup role changed during verification"
               and not cell.completions, (s.status, s.failure))
    finally:
        cell.close()

cell = Cell(worker="live")
try:
    wait_for(lambda: cell.native() is not None and children_of(cell.native()))
    s = cell.scout()
    real_read = s.read

    def native_fdinfo(path, directory=None):
        data = real_read(path, directory)
        if path.startswith("/proc/self/fdinfo/"):
            line = [x for x in data.splitlines() if x.startswith(b"Pid:\t")][0]
            if is_native(int(line.split(b"\t")[1])):
                return data.replace(line, b"Pid:\t-1")
        return data
    s.read = native_fdinfo
    s.run()
    expect("native-fdinfo-minus-one", s.status == "failed" and s.failure == "startup unpaired pidfd"
           and not s.reaped_mid_bind, (s.status, s.failure))
finally:
    cell.close()

# The holder publishes one completion, never after a retention (review-2 J14).
custody.admit_startup_completion(None, {})
for label, args in (("completion-after-retention", (("k",), {})),
                    ("completion-beside-held-pair", (None, {("k",): 1}))):
    try:
        custody.admit_startup_completion(*args)
    except ValueError as error:
        expect(label, "startup completion after retention" in str(error), error)
    else:
        raise SystemExit(f"FAIL {label} accepted")

# 10. The holder never retains a live pair as a completed transient role.
live = subprocess.Popen(["sleep", "30"])
try:
    fd = os.pidfd_open(live.pid)
    try:
        expect("holder-refuses-live-pidfd-as-transient", custody.retainable_startup_state(
            fd, dict(state="Z", completedTransient=dict(exitCode=0, role="__zfb-compiler-worker"))) is False)
        expect("holder-retains-live-worker", custody.retainable_startup_state(fd, dict(state="S")) is True)
    finally:
        os.close(fd)
finally:
    live.kill()
    live.wait()

# 11. The completed-role predicate on exact stat inputs (no process needed).
probe = scout.Startup(dict(launch=dict(cwd="/"), root={}, child={}), 1, dict(bootId="b"), lambda *a: None)
parent = dict(pid=200, startTicks="1000")
image = dict(path="/opt/zfb/zfb")


def stat(pid=300, comm=b"zfb", state=b"Z", ppid=200, pgid=300, ticks=1001, exit_code=0, fields=52):
    values = [state, str(ppid).encode(), str(pgid).encode()] + [b"0"] * (fields - 5)
    values[19] = str(ticks).encode()
    if fields >= 52:
        values[49] = str(exit_code).encode()
    return str(pid).encode() + b" (" + comm + b") " + b" ".join(values) + b"\n"


procdir = os.open("/proc/self", os.O_RDONLY | os.O_DIRECTORY)


def completed(raw, pid=300, ppid=200, pgid=300):
    head, tail = raw.rsplit(b")", 1)
    try:
        probe.completed((None, procdir), raw, head, tail.split(), pid, ppid, pgid, image, parent)
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
    ("predicate-exit-unreadable", stat(fields=51), {}, "startup transient exit status unreadable"),
    ("predicate-exit-nonzero", stat(exit_code=256), {}, "startup transient compiler exited unsuccessfully"),
):
    expect(label, completed(raw, **kwargs) == message, completed(raw, **kwargs))
os.close(procdir)

print("PASS " + " ".join(results))
