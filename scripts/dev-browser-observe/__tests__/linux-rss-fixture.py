"""Barrier-driven Linux RSS sampler fixtures. One root and one separate-group child."""

import ctypes
import importlib.util
import io
import json
import os
from pathlib import Path
import resource
import select
import signal
import socket
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch

here = Path(__file__).resolve()
spec = importlib.util.spec_from_file_location("rss", here.parents[1] / "lib/linux-rss-sample.py")
rss = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rss)


def fixture():
    signal.alarm(8)
    resource.setrlimit(resource.RLIMIT_AS, (128 << 20, 128 << 20))
    gate_r, gate_w = os.pipe()
    ready_r, ready_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(gate_w)
        os.close(ready_r)
        os.setpgid(0, 0)
        def worker():
            pages = bytearray(1 << 20)
            os.write(ready_w, b"r")
            os.read(gate_r, 1)
            os._exit(0)
        if sys.argv[-1] == "leader-exit":
            threading.Thread(target=worker).start()
            ctypes.CDLL(None).pthread_exit(None)
        else:
            worker()
    os.close(gate_r)
    os.close(ready_w)
    os.read(ready_r, 1)
    os.close(ready_r)
    print(pid, flush=True)
    try:
        for command in sys.stdin:
            if command.strip() == "release":
                os.write(gate_w, b"x")
                # WNOWAIT proves zombie state while the test deliberately holds it.
                os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)
            elif command.strip() == "reap":
                os.waitpid(pid, 0)
            print("ok", flush=True)
    finally:
        os.close(gate_w)
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass


def identity(pid):
    row = rss.stat_row(Path(f"/proc/{pid}/stat").read_bytes(), pid)
    return dict(row, bootId=Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                birthVersion="linux-proc-v1")


class CustodyMessages(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("fixed_custody", here.parents[1] / "lib/linux-rss-custody.py")
        cls.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.module)

    def test_truncated_wrong_count_and_invalid_json_close_every_received_fd(self):
        import array
        left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        with open("/dev/null", "rb") as source:
            before = set(os.listdir("/proc/self/fd"))
            for data, count in [(b"{}", 1), (b"{}", 24), (b"x", 2), (b"x"*4096, 2)]:
                left.sendmsg([data], [(socket.SOL_SOCKET,socket.SCM_RIGHTS,array.array("i",[source.fileno()]*count))])
                with self.assertRaises(ValueError):
                    self.module.receive(right, [2])
                self.assertEqual(set(os.listdir("/proc/self/fd")), before)

    def test_retrieval_rechecks_both_received_descriptor_identities_and_types(self):
        marker = self.module.Client.__new__(self.module.Client)
        marker.tokens = {"12": "token"}
        with open("/dev/null", "rb") as source:
            for identities in ([["0","0"],["0","0"]], [self.module.identity(source.fileno())]*2):
                fds=[os.dup(source.fileno()),os.dup(source.fileno())]
                with patch.object(marker, "exchange", return_value=({"descriptors":identities},fds)):
                    with self.assertRaisesRegex(ValueError, "retrieved descriptor mismatch"):
                        marker.retrieve({"pid":12})
                for fd in fds:
                    with self.assertRaises(OSError):
                        os.fstat(fd)

    def test_ack_requires_original_session_epoch_holder_and_helper(self):
        marker = self.module.Client.__new__(self.module.Client)
        marker.session,marker.epoch,marker.holder_pid = "original", 4, 151
        marker.peer, other = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.addCleanup(marker.peer.close)
        self.addCleanup(other.close)
        valid = dict(ok=True,session="original",epoch=4,holderPid=151,helperPid=os.getpid(),verifiedNs="1")
        for wrong in (dict(session="prior"),dict(epoch=3),dict(holderPid=150),dict(helperPid=1),dict(ok=False)):
            self.module.send(other,dict(valid, **wrong))
            with self.assertRaisesRegex(ValueError,"unverified custody ACK"):
                marker.exchange(dict(op="test"))
            self.module.receive(other,[0])
        other.close()
        with self.assertRaises(OSError):
            marker.exchange(dict(op="test"))


class Sampling(unittest.TestCase):
    def spawn(self, mode="ordinary", fixed=False):
        self.start_ns = time.monotonic_ns()
        argv = ([os.path.realpath(sys.executable), str(here)] if fixed else
                [sys.executable, "-B", __file__])
        # Fixed role verification needs Python and the script as argv[0:2].
        # Preserve that shape while keeping the fixture import out of the source tree.
        child = subprocess.Popen([*argv, "fixture", mode],
                                 env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"} if fixed else None,
                                 start_new_session=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 text=True)
        self.addCleanup(self.close, child)
        self.child = child
        self.pid = int(child.stdout.readline())
        root = identity(child.pid)
        stat = os.stat(sys.executable)
        self.request = dict(root=root, known=[root], terminalProofs=[],
                            executable=dict(path=os.path.realpath(sys.executable),
                                            device=str(stat.st_dev), inode=str(stat.st_ino)))
        if fixed:
            info = here.stat()
            self.request.update(fixedDevArgs=["fixture", mode], fixedChildExecutable=self.request["executable"],
                                fixedLaunch=dict(cwd=os.getcwd(), shim=dict(path=str(here),
                                device=str(info.st_dev), inode=str(info.st_ino))))
        self.events = []
        self.sample = rss.Sample(self.request, self.events.append)

    def close(self, child):
        child.stdin.close()
        self.assertEqual(child.wait(timeout=8), 0)
        child.stdout.close()
        for pid in [child.pid, self.pid]:
            self.assertFalse(Path(f"/proc/{pid}").exists(), f"remaining process {pid}")

        if "--receipts" in sys.argv:
            print(json.dumps(dict(test=self._testMethodName, root=child.pid,
                                  separateGroupChild=self.pid, bothAbsent=True,
                                  addressSpaceLimitMiB=128, wallLimitMs=8000,
                                  elapsedMs=(time.monotonic_ns()-self.start_ns)/1e6)))

    def command(self, cmd):
        self.child.stdin.write(cmd + "\n")
        self.child.stdin.flush()
        self.assertEqual(self.child.stdout.readline(), "ok\n")

    def outcome(self):
        try:
            self.sample.run()
        except (OSError, ValueError, rss.Invalid) as error:
            self.events.append(self.sample.problem(error))
        return self.events[-1]

    def observe_child(self, fn):
        original = self.sample.observe
        def observe(binding):
            if binding[0]["pid"] == self.pid:
                fn()
            return original(binding)
        return patch.object(self.sample, "observe", observe)

    def test_separate_group_live_rss(self):
        self.spawn()
        self.assertEqual(self.outcome()["type"], "complete")
        obs = [e["process"] for e in self.events if e["type"] == "observation"]
        self.assertEqual({p["pid"] for p in obs}, {self.pid, self.child.pid})
        self.assertTrue(all(p["rssKiB"] > 0 and p["disposition"] == "live" for p in obs))
        self.assertEqual(obs[-1]["pgid"], self.pid)

    def test_fixed_early_bind_precedes_reaping_after_discovery(self):
        self.spawn(fixed=True)
        self.command("release")
        self.sample.fixed_child = self.request["executable"]
        self.sample.fixed_workers = True
        rows = self.sample.rows
        def reap_after_row(directory, cap):
            for pid in rows(directory, cap):
                yield pid
                if directory == "/proc" and pid == self.pid:
                    self.command("reap")
        with patch.object(self.sample, "rows", reap_after_row):
            self.assertEqual(self.outcome()["type"], "complete")
        child = next(e["process"] for e in self.events
                     if e["type"] == "observation" and e["process"]["pid"] == self.pid)
        self.assertEqual(child["disposition"], "proved-departure")
        self.assertIsNone(child["rssKiB"])
        self.assertEqual(child["fixedRole"], "unknown")

    def test_captured_wrong_live_role_remains_invalid_after_exit(self):
        self.spawn(fixed=True)
        self.sample.fixed_child = self.request["executable"]
        self.sample.fixed_workers = True
        real_identity = self.sample.exe_identity
        def exit_with_error(directory, row):
            try:
                return real_identity(directory, row)
            except rss.Invalid:
                self.command("release")
                raise
        with patch.object(self.sample, "exe_identity", exit_with_error):
            outcome = self.outcome()
        self.assertEqual(outcome["error"], "unexpected-fixed-native-group")
        self.assertIsNotNone(outcome["terminalProof"])
        self.assertFalse(any(e.get("process", {}).get("pid") == self.pid for e in self.events))

    def test_fixed_live_compiler_identity_and_private_argv(self):
        self.spawn()
        exe = self.request["executable"]
        self.sample.fixed_child = exe
        self.sample.fixed_workers = True
        native = dict(identity(self.pid), pgid=self.child.pid)
        worker = dict(pid=600, pgid=600, ppid=self.pid)
        directory = os.open(f"/proc/{self.pid}", rss.DIR)
        self.addCleanup(os.close, directory)
        self.sample.fixed_dev_args = ["dev", "--port", "12345"]
        dev = b"\0".join(os.fsencode(arg) for arg in [exe["path"], *self.sample.fixed_dev_args]) + b"\0"
        with patch.object(self.sample, "read", return_value=dev):
            self.assertEqual(self.sample.exe_identity(directory, native), exe)
        with patch.object(self.sample, "read", return_value=dev+b"extra\0"):
            with self.assertRaisesRegex(rss.Invalid, "unexpected-fixed-dev-argv"):
                self.sample.exe_identity(directory, native)
        argc = os.fsencode(exe["path"]) + b"\0__zfb-compiler-worker\0"
        with patch.object(self.sample, "read", return_value=argc):
            actual = self.sample.exe_identity(directory, worker)
            self.assertEqual(actual["workerArg"], "__zfb-compiler-worker")
            with self.assertRaisesRegex(rss.Invalid, "unexpected-fixed-process-role"):
                self.sample.exe_identity(directory, dict(worker, ppid=601))
            with self.assertRaisesRegex(rss.Invalid, "unexpected-fixed-process-role"):
                self.sample.exe_identity(directory, dict(worker, pgid=601))
        for bad in (argc[:-1], argc+b"extra\0", b"other\0__zfb-compiler-worker\0"):
            with patch.object(self.sample, "read", return_value=bad):
                with self.assertRaisesRegex(rss.Invalid, "unexpected-fixed-compiler-argv"):
                    self.sample.exe_identity(directory, worker)
        with patch.object(os, "readlink", return_value="/different"):
            with self.assertRaisesRegex(rss.Invalid, "unexpected-executable"):
                self.sample.exe_identity(directory, worker)

    def test_final_root_and_cleanup_are_independently_bracketed(self):
        self.spawn()
        now = 1_000_000
        spans = {}
        def record(stage, ns):
            spans[stage] = ns
        self.sample.timing = record
        check = self.sample.check_root
        def checked(bound):
            nonlocal now
            if "finalRootBegin" in spans:
                now += 43_000_000
            return check(bound)
        close = rss.contextlib.ExitStack.close
        def closed(stack):
            nonlocal now
            now += 37_000_000
            return close(stack)
        with patch.object(rss, "CLOCK", side_effect=lambda: now), \
             patch.object(self.sample, "check_root", checked), \
             patch.object(rss.contextlib.ExitStack, "close", closed):
            self.assertEqual(self.outcome()["type"], "complete")
        self.assertEqual(spans["finalRoot"][1]-spans["finalRoot"][0], 43_000_000)
        self.assertEqual(spans["fdCleanup"][1]-spans["fdCleanup"][0], 37_000_000)
        self.assertLessEqual(spans["finalRoot"][1], spans["fdCleanup"][0])

    def test_observation_and_terminal_output_time_the_actual_writer(self):
        now = 1
        class Writer(io.BytesIO):
            def write(self, data):
                nonlocal now
                now += 17
                return super().write(data)
            def flush(self):
                nonlocal now
                now += 23
                return super().flush()
        stream = Writer()
        journal = rss.Journal(stream)
        encode = json.dumps
        def serialize(*args, **kwargs):
            nonlocal now
            now += 11
            return encode(*args, **kwargs)
        with patch.object(rss, "CLOCK", side_effect=lambda: now), \
             patch.object(json, "dumps", serialize):
            journal.emit(dict(type="observation", process=dict(pid=789)))
            journal.emit(dict(type="complete", _preparedNs="1", _preparedEndNs="1"))
        rows = [json.loads(line) for line in stream.getvalue().splitlines()]
        for frame in (rows[1], rows[3]):
            a, b, c, d = map(int, frame["ns"][-4:])
            self.assertEqual([b-a, c-b, d-c], [11, 17, 23])
        self.assertEqual(journal.bytes, len(stream.getvalue()))
        self.assertEqual([r["stage"] for r in rows[1::2]], ["observationOutput", "terminalOutput"])

    def test_exit_before_binding(self):
        self.spawn()
        original = self.sample.bind
        def bind(stack, candidate):
            if candidate["pid"] == self.pid:
                self.command("release")
                self.command("reap")
            return original(stack, candidate)
        with patch.object(self.sample, "bind", bind):
            self.assertEqual(self.outcome()["error"], "unbound-candidate-departure")
        self.assertFalse(any(e["type"] == "bound" and e["birth"]["pid"] == self.pid for e in self.events))

    def test_bound_unreaped(self):
        self.spawn()
        with self.observe_child(lambda: self.command("release")):
            self.assertEqual(self.outcome()["type"], "complete")
        terminated = self.events[-2]["process"]
        self.assertEqual(terminated["disposition"], "proved-departure")
        self.assertEqual(terminated["terminalProof"]["pollMask"], select.POLLIN)
        self.assertIsNone(terminated["rssKiB"])
        self.request["known"].append(terminated)
        self.request["terminalProofs"].append(terminated["terminalProof"])
        self.command("reap")
        self.sample = rss.Sample(self.request, self.events.append)
        self.assertEqual(self.outcome()["type"], "complete")
        self.assertTrue(any(e.get("process", {}).get("disposition") == "retained-terminal-proof"
                            for e in self.events))

    def test_reaped_after_verified_rss(self):
        self.spawn()
        def emit(event):
            self.events.append(event)
            if event["type"] == "rss" and event["birth"]["pid"] == self.pid:
                self.command("release")
                self.command("reap")
        self.sample.emit = emit
        self.assertEqual(self.outcome()["type"], "complete")
        terminated = self.events[-2]["process"]
        self.assertEqual(terminated["disposition"], "proved-departure")
        self.assertGreater(terminated["rssKiB"], 0)
        self.assertIsNone(terminated["smapsRollupKiB"])
        self.assertIn(terminated["terminalProof"]["pollMask"], (16, 17))

    def test_leader_exited_live_worker(self):
        self.spawn("leader-exit")
        deadline = time.monotonic() + 2
        while identity(self.pid)["state"] != "Z" and time.monotonic() < deadline:
            time.sleep(.001)
        self.assertEqual(identity(self.pid)["state"], "Z")
        self.assertEqual(len(os.listdir(f"/proc/{self.pid}/task")), 2)
        fd = os.pidfd_open(self.pid, 0)
        try:
            self.assertIsNone(rss.terminal(fd))
        finally:
            os.close(fd)
        self.assertEqual(self.outcome()["type"], "failure")
        self.assertFalse(any(e.get("process", {}).get("disposition") == "proved-departure"
                             for e in self.events))

    def test_tombstone_does_not_authorize_reuse_or_pgid_change(self):
        self.spawn()
        for field in ("startTicks", "pgid", "bootId"):
            with self.subTest(field=field):
                old = identity(self.pid)
                old[field] = old[field]+1 if field == "pgid" else "wrong"
                request = dict(self.request, known=[self.request["root"], old],
                               terminalProofs=[dict(birth=old, pidfdFlags=0, pollMask=1, observedNs="1")])
                self.sample = rss.Sample(request, self.events.append)
                self.assertEqual(self.outcome()["error"], "birth-or-group-mismatch")

    def test_foreign_path_cannot_be_hidden_by_exit_during_inode_stat(self):
        self.spawn()
        with patch.object(os, "readlink", return_value="/foreign"), patch.object(os, "stat",
                side_effect=ProcessLookupError) as inode:
            with self.assertRaisesRegex(rss.Invalid, "unexpected-executable"):
                self.sample.exe_identity(123)
            inode.assert_not_called()

    def test_real_descriptor_exhaustion_after_verified_handoff(self):
        self.spawn()
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        def emit(row):
            self.events.append(row)
            if row["type"] == "bound" and row["birth"]["pid"] == self.pid:
                resource.setrlimit(resource.RLIMIT_NOFILE, (3, hard))
        self.sample.emit = emit
        try:
            result = self.outcome()
        finally:
            resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))
        self.assertEqual(result["errno"], 24)
        self.assertEqual(result["syscallError"], "EMFILE")
        self.assertTrue(any(e["type"] == "bound" and e["birth"]["pid"] == self.pid for e in self.events))

    def test_independent_verified_birth_retained_when_other_known_child_disappears(self):
        self.spawn()
        missing = dict(self.sample.root, pid=2**31-1)
        self.sample.known[missing["pid"]] = missing
        self.assertEqual(self.outcome()["error"], "unbound-known-departure")
        self.assertTrue(any(e["type"] == "bound" and e["birth"]["pid"] == self.pid for e in self.events))

    def test_permission_descriptor_and_malformed_proc_reject(self):
        self.spawn()
        for exc in (PermissionError(13, "permission"), OSError(24, "open files"),
                    rss.Invalid("malformed-proc-counter")):
            with self.subTest(error=str(exc)):
                with patch.object(self.sample, "exe_identity", side_effect=exc):
                    self.assertEqual(self.outcome()["type"], "failure")
                    self.assertTrue(any(e["type"] == "bound" and e["birth"]["pid"] == self.pid
                                        for e in self.events))
        with self.assertRaises(rss.Invalid):
            rss.counters(b"Rss: 2 kB\nRss: 3 kB\n", rss.ROLLUP)
        with self.assertRaises(rss.Invalid):
            rss.stat_row(b"123 (x) D 0\n", 123)

    def test_stale_intermediate_parent_never_grants_ownership(self):
        self.spawn()
        root = self.request["root"]
        parent = identity(self.pid)
        grandchild = dict(parent, pid=self.pid+1, ppid=self.pid)
        other = dict(parent, startTicks=parent["startTicks"]+"0")
        with patch.object(self.sample, "stat", return_value=other):
            with self.assertRaisesRegex(rss.Invalid, "unverified-ancestry"):
                self.sample.verify_chain(grandchild, {parent["pid"]: (parent, None, None)})

    def test_root_lost_while_real_separate_child_remains(self):
        # Let the test parent reap its own orphan after the barrier-controlled
        # root exits. The sampling helper itself never signals or reaps.
        libc = ctypes.CDLL(None, use_errno=True)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)  # PR_SET_CHILD_SUBREAPER
        gate_r, gate_w = os.pipe()
        script = """
import os, sys, signal, resource
resource.setrlimit(resource.RLIMIT_AS,(128<<20,128<<20))
signal.alarm(6)
p = os.fork()
if p:
    print(p, flush=True)
    sys.stdin.buffer.read(1)
    os._exit(0)
os.setpgid(0,0)
os.read(int(sys.argv[1]), 1)
os._exit(0)
"""
        root = subprocess.Popen([sys.executable, "-c", script, str(gate_r)],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                start_new_session=True, pass_fds=(gate_r,))
        os.close(gate_r)
        pid = int(root.stdout.readline())
        try:
            birth = identity(root.pid)
            child = identity(pid)
            root.stdin.close()
            self.assertEqual(root.wait(timeout=2), 0)
            self.assertEqual(identity(pid)["startTicks"], child["startTicks"])
            self.events = []
            self.sample = rss.Sample(dict(root=birth, known=[birth, child],
                                      terminalProofs=[], executable={}), self.events.append)
            result = self.outcome()
            self.assertEqual(result["type"], "failure")
            self.assertEqual(result["expected"]["pid"], root.pid)
        finally:
            os.close(gate_w)
            self.assertEqual(os.waitpid(pid, 0), (pid, 0))
            root.stdout.close()
            self.assertFalse(Path(f"/proc/{pid}").exists())
            self.assertEqual(libc.prctl(36, 0, 0, 0, 0), 0)
            if "--receipts" in sys.argv:
                print(json.dumps(dict(test=self._testMethodName, root=root.pid,
                                      separateGroupChild=pid, bothAbsent=True,
                                      addressSpaceLimitMiB=128, wallLimitMs=6000)))

    def test_independent_counter_survives_unrelated_malformed_field(self):
        self.spawn()
        read = self.sample.read
        def malformed(path, cap, directory=None):
            if path == "status":
                return b"VmRSS: 4300000 kB\nVmSwap: bogus\n"
            return read(path, cap, directory)
        with patch.object(self.sample, "read", malformed):
            self.assertEqual(self.outcome()["error"], "malformed-proc-counter")
        self.assertTrue(any(e["type"] == "rss" and e["rssKiB"] == 4300000
                            for e in self.events))

    def test_selection(self):
        root = dict(pid=12, pgid=12)
        rows = [dict(pid=p, pgid=g, ppid=pp) for p,g,pp in
                [(30,30,20), (40,40,90), (20,20,12), (50,12,1), (12,12,1)]]
        self.assertEqual(set(rss.owned(rows, root)), {12,20,30,50})


def instrumented(mode):
    # The actual helper protocol, with controlled delays/faults in fixtures only.
    signal.alarm(5)
    sys.argv = [sys.argv[0], str(os.getppid())]
    emit_timing = rss.Journal.timing
    def timing(journal, stage, ns, **fields):
        if stage == "request" and mode == "other-namespace":
            fields["clock"]["timeNamespace"] = "time:[0]"
        elif stage == "request" and mode == "bad-envelope":
            ns = [0, *ns[1:]]
        elif stage == "preExit" and mode == "missing-pre-exit":
            return
        return emit_timing(journal, stage, ns, **fields)
    with patch.object(rss.Journal, "timing", timing):
        if mode == "semantic-failure":
            with patch.object(rss.Sample, "threads", side_effect=PermissionError(13,"test")):
                return rss.main()
        code = rss.main()
    if mode == "slow-exit":
        time.sleep(.055)
    if mode.startswith("held-"):
        # Transfer a stdout writer to a second direct child of the test runner.
        # Ack ensures custody before this helper exits. The runner reaps both.
        with socket.socket(fileno=3) as ctl:
            ctl.setblocking(True)
            socket.send_fds(ctl, [b'f'], [sys.stdout.fileno()])
            assert ctl.recv(1) == b'a'
    return code


def hold_stdout():
    signal.alarm(5)
    resource.setrlimit(resource.RLIMIT_AS, (96 << 20, 96 << 20))
    with socket.socket(fileno=3) as ctl:
        ctl.setblocking(True)
        _, writers, *_ = socket.recv_fds(ctl, 1, 1)
        assert len(writers) == 1
        ctl.sendall(b'a')
    try:
        sys.stdin.buffer.read(1)
    finally:
        os.close(writers[0])


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "fixture":
        fixture()
    elif sys.argv[1:2] == ["instrumented"]:
        sys.exit(instrumented(sys.argv[2]))
    elif sys.argv[1:2] == ["hold-stdout"]:
        hold_stdout()
    else:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
