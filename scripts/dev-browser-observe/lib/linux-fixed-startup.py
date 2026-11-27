"""One bounded, identity-only lookup of the fixed Linux startup compiler."""

import contextlib
import os
import select
import threading
import time

# zfb's config bundler (`zfb __zfb-compiler-worker`, crates/zfb-compiler-worker)
# can exit before the first 20 ms sweep reaches it (#4862 r4 slot 01). Only that
# role may be first seen terminal, and only as a zombie that exited with status 0.
COMPILER_WORKER_ARG = "__zfb-compiler-worker"
STAT_EXIT_CODE = 49  # /proc/<pid>/stat field 52, counted after the ")" of comm


class Startup:
    def __init__(self, config, parent, seed, publish):
        self.config = config
        self.parent = parent
        self.seed = seed
        self.publish = publish
        self.stop = threading.Event()
        self.reads = self.bytes = self.sweeps = 0
        self.start_ns = time.monotonic_ns()
        self.end_ns = self.start_ns + 3_000_000_000
        self.status = "running"
        self.failure = None
        self.thread = threading.Thread(target=self.run, daemon=True)

    def read(self, path, directory=None):
        if self.reads >= 8192 or self.bytes >= 4 << 20:
            raise ValueError("startup proc input cap")
        fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC, dir_fd=directory)
        try:
            data = os.read(fd, min(16385, (4 << 20) - self.bytes + 1))
        finally:
            os.close(fd)
        self.reads += 1
        self.bytes += len(data)
        if len(data) > 16384 or self.bytes > 4 << 20:
            raise ValueError("startup proc field cap")
        return data

    def children(self, directory):
        tasks = os.open("task", os.O_RDONLY | os.O_DIRECTORY, dir_fd=directory)
        try:
            children = set()
            with os.scandir(tasks) as entries:
                for count, entry in enumerate(entries, 1):
                    if count > 128:
                        raise ValueError("startup task cap")
                    try:
                        children.update(
                            map(int, self.read(entry.name + "/children", tasks).split())
                        )
                    except FileNotFoundError:
                        continue
                    if len(children) > 32:
                        raise ValueError("startup children cap")
            return children
        finally:
            os.close(tasks)

    def verify(self, pair, pid, ppid, pgid, pin, argv, transient_parent=None):
        process, directory = pair
        raw = self.read("stat", directory)
        head, tail = raw.rsplit(b")", 1)
        state, parent, group, *rest = tail.split()
        if (
            int(head.split(b"(", 1)[0]) != pid
            or int(parent) != ppid
            or int(group) != pgid
        ):
            raise ValueError("startup process ancestry mismatch")
        if state in (b"Z", b"X", b"x"):
            if transient_parent is None:
                raise ValueError("startup role first seen terminal")
            return self.completed(process, raw, head, state, rest, pid, ppid, pgid, pin,
                                  argv, transient_parent)
        fdinfo = self.read(f"/proc/self/fdinfo/{process}").splitlines()
        if b"Pid:\t" + str(pid).encode() not in fdinfo:
            raise ValueError("startup unpaired pidfd")
        if (
            self.read("cmdline", directory)
            != b"\0".join(map(os.fsencode, argv)) + b"\0"
        ):
            raise ValueError("startup process argv mismatch")
        if os.readlink("cwd", dir_fd=directory) != self.config["launch"]["cwd"]:
            raise ValueError("startup process cwd mismatch")
        exe = os.open("exe", os.O_RDONLY, dir_fd=directory)
        try:
            info = os.fstat(exe)
            if (
                os.readlink("exe", dir_fd=directory),
                str(info.st_dev),
                str(info.st_ino),
                os.read(exe, 4),
            ) != (pin["path"], pin["device"], pin["inode"], b"\x7fELF"):
                raise ValueError("startup process image mismatch")
        finally:
            os.close(exe)
        poller = select.poll()
        poller.register(process, select.POLLIN)
        if poller.poll(0):
            raise ValueError("startup role exited during binding")
        return dict(
            pid=pid,
            ppid=ppid,
            pgid=pgid,
            bootId=self.seed["bootId"],
            startTicks=rest[16].decode(),
            birthVersion="linux-proc-v1",
            state=state.decode(),
            comm=head.split(b"(", 1)[1].decode(),
        )

    def completed(self, process, raw, head, state, rest, pid, ppid, pgid, pin, argv, parent):
        """A compiler worker already exited at first sight: identity from stat only.

        A zombie has no readable cmdline, cwd or exe. Its private group, the
        exact zfb parent birth, its image name and a zero exit status bind it.
        """
        comm = head.split(b"(", 1)[1]
        fields = [state, b"", b"", *rest]
        if (
            state != b"Z"
            or argv != [pin["path"], COMPILER_WORKER_ARG]
            or pgid != pid
            or ppid != parent["pid"]
            or comm != os.path.basename(pin["path"]).encode()[:15]
            or int(rest[16]) < int(parent["startTicks"])
        ):
            raise ValueError("startup role first seen terminal")
        if len(fields) <= STAT_EXIT_CODE:
            raise ValueError("startup transient exit status unreadable")
        exit_code = int(fields[STAT_EXIT_CODE])
        if exit_code != 0:
            raise ValueError("startup transient compiler exited unsuccessfully")
        fdinfo = self.read(f"/proc/self/fdinfo/{process}").splitlines()
        if b"Pid:\t" + str(pid).encode() not in fdinfo:
            raise ValueError("startup unpaired pidfd")
        poller = select.poll()
        poller.register(process, select.POLLIN)
        if not poller.poll(0):
            raise ValueError("startup transient pidfd not terminal")
        return dict(
            pid=pid,
            ppid=ppid,
            pgid=pgid,
            bootId=self.seed["bootId"],
            startTicks=rest[16].decode(),
            birthVersion="linux-proc-v1",
            state="Z",
            comm=comm.decode(),
            completedTransient=dict(
                role=COMPILER_WORKER_ARG,
                exitCode=exit_code,
                stat=raw.decode().rstrip("\n"),
                observedNs=str(time.monotonic_ns()),
                parent=dict(pid=parent["pid"], startTicks=parent["startTicks"]),
            ),
        )

    def sweep(self):
        root = self.seed
        argv = root["argv"]
        fixed = self.config
        shim = fixed["launch"]["shim"]
        info = os.stat(shim["path"])
        if (str(info.st_dev), str(info.st_ino)) != (shim["device"], shim["inode"]):
            raise ValueError("startup shim inode mismatch")
        if not argv or argv[0] != shim["path"]:
            raise ValueError("unexpected startup distribution shim")
        if (
            self.read("/proc/sys/kernel/random/boot_id").decode().strip()
            != root["bootId"]
        ):
            raise ValueError("startup root boot mismatch")
        with contextlib.ExitStack() as cleanup:

            def bind(pid, ppid, group, image, args, transient_parent=None):
                process = os.pidfd_open(pid, 0)
                cleanup.callback(os.close, process)
                directory = os.open(f"/proc/{pid}", os.O_RDONLY | os.O_DIRECTORY)
                cleanup.callback(os.close, directory)
                pair = (process, directory)
                inputs = (pair, pid, ppid, group, image, args)
                return pair, self.verify(*inputs, transient_parent), inputs

            r, rb, ri = bind(
                root["pid"],
                self.parent,
                root["pid"],
                fixed["root"],
                [fixed["root"]["path"], *argv],
            )
            if rb["startTicks"] != root["startTicks"]:
                raise ValueError("startup root birth mismatch")
            native = self.children(r[1])
            if not native:
                return
            if len(native) != 1:
                raise ValueError("ambiguous startup dev child")
            v, vb, vi = bind(
                native.pop(),
                rb["pid"],
                rb["pgid"],
                fixed["child"],
                [fixed["child"]["path"], *argv[1:]],
            )
            workers = self.children(v[1])
            if not workers:
                return
            if len(workers) != 1:
                raise ValueError("ambiguous startup compiler child")
            pid = workers.pop()
            w, wb, wi = bind(
                pid,
                vb["pid"],
                pid,
                fixed["child"],
                [fixed["child"]["path"], COMPILER_WORKER_ARG],
                vb,
            )
            roles = ((rb, ri), (vb, vi))
            if "completedTransient" in wb:
                # The pidfd is bound to that exact exited process; zfb may reap it
                # at any moment, so its procdir is not read again.
                poller = select.poll()
                poller.register(w[0], select.POLLIN)
                if not poller.poll(0):
                    raise ValueError("startup role changed during verification")
            else:
                roles += ((wb, wi),)
            for prior, inputs in roles:
                after = self.verify(*inputs)
                if any(
                    prior[k] != after[k]
                    for k in ("pid", "ppid", "pgid", "bootId", "startTicks")
                ):
                    raise ValueError("startup role changed during verification")
            if time.monotonic_ns() >= self.end_ns or self.stop.is_set():
                raise ValueError("startup live handoff expired")
            self.publish(w, wb, vb, rb)
            self.status = "retained"

    def run(self):
        try:
            while (
                not self.stop.is_set()
                and self.sweeps < 150
                and time.monotonic_ns() < self.end_ns
            ):
                self.sweeps += 1
                self.sweep()
                if self.status == "retained":
                    break
                self.stop.wait(0.020)
            if self.status != "retained":
                self.status = "missed"
        except Exception as error:
            self.status = "failed"
            self.failure = str(error)
        finally:
            self.finished_ns = time.monotonic_ns()
            self.publish(None, None, None, None)

    def report(self):
        return dict(
            status=self.status,
            error=self.failure,
            sweeps=self.sweeps,
            reads=self.reads,
            procBytes=self.bytes,
            startNs=str(self.start_ns),
            deadlineNs=str(self.end_ns),
            finishedNs=str(self.finished_ns),
        )
