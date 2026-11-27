"""One bounded, identity-only lookup of the fixed Linux startup compiler."""

import contextlib
import os
import select
import threading
import time

# zfb's config bundler (`zfb __zfb-compiler-worker`, crates/zfb-compiler-worker)
# lives tens of ms, and zfb 6.0.1 reaps it within one 5 ms WNOWAIT poll of its
# exit (parent.rs:23,483-489), so a 20 ms sweep sees it live, as a zombie, or not
# at all (#4862 r4 slot 01). Each is a defined outcome; see DESIGN in sweep().
COMPILER_WORKER_ARG = "__zfb-compiler-worker"
# /proc/<pid>/stat fields counted after the ")" of comm: field N is index N - 3.
STAT_CMINFLT = 8  # field 11: minor faults of children this process has waited for
STAT_EXIT_CODE = 49  # field 52
TRACE_CAP = 16


class WorkerGone(Exception):
    """The compiler worker was reaped between being listed and being bound."""


class MaybeEnded(ValueError):
    """A worker-slot answer that an exited process also gives: empty argv or a
    terminal pidfd. Any other mismatch is real negative evidence and refuses."""


class Startup:
    def __init__(self, config, parent, seed, publish, complete=None):
        self.config = config
        self.parent = parent
        self.seed = seed
        self.publish = publish
        self.complete = complete
        self.trace = []
        self.reaped_mid_bind = []
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
        try:
            raw = self.read("stat", directory)
        except ProcessLookupError:
            if transient_parent is None:
                raise
            raise WorkerGone("stat-esrch")
        head, tail = raw.rsplit(b")", 1)
        fields = tail.split()
        state, parent, group, *rest = fields
        if (
            int(head.split(b"(", 1)[0]) != pid
            or int(parent) != ppid
            or int(group) != pgid
        ):
            raise ValueError("startup process ancestry mismatch")
        if state in (b"Z", b"X", b"x"):
            if transient_parent is None:
                raise ValueError("startup role first seen terminal")
            return self.completed(pair, raw, head, fields, pid, ppid, pgid, pin, transient_parent)
        try:
            self.paired(process, pid, transient_parent is not None)
            cmdline = self.read("cmdline", directory)
            if cmdline != b"\0".join(map(os.fsencode, argv)) + b"\0":
                raise (MaybeEnded if cmdline == b"" else ValueError)("startup process argv mismatch")
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
                raise MaybeEnded("startup role exited during binding")
        except (MaybeEnded, FileNotFoundError, ProcessLookupError) as error:
            # A zombie's cmdline reads empty and its cwd/exe raise ENOENT; a reaped
            # process answers ESRCH. Only the worker slot may have ended here. A
            # non-empty foreign argv, or a readable cwd or exe that differs, refuses
            # even if the process has exited since (review-3 finding 1).
            if transient_parent is None:
                raise
            return self.exited(pair, pid, ppid, pgid, pin, transient_parent, error)
        return dict(
            pid=pid,
            ppid=ppid,
            pgid=pgid,
            bootId=self.seed["bootId"],
            startTicks=fields[19].decode(),
            birthVersion="linux-proc-v1",
            state=state.decode(),
            comm=head.split(b"(", 1)[1].decode(),
            cminflt=int(fields[STAT_CMINFLT]),
        )

    def paired(self, process, pid, worker):
        fdinfo = self.read(f"/proc/self/fdinfo/{process}").splitlines()
        if b"Pid:\t" + str(pid).encode() in fdinfo:
            return
        if worker and b"Pid:\t-1" in fdinfo:
            raise WorkerGone("fdinfo-pid-minus-one")
        raise ValueError("startup unpaired pidfd")

    def exited(self, pair, pid, ppid, pgid, pin, parent, error):
        """The worker ended after being listed live: route it by its pidfd and stat.

        A terminal pidfd with a status-0 zombie is a completed transient role; a
        stat read that answers ESRCH means zfb already reaped it. A live pidfd
        keeps the original refusal.
        """
        poller = select.poll()
        poller.register(pair[0], select.POLLIN)
        if not poller.poll(0):
            raise error
        try:
            raw = self.read("stat", pair[1])
        except ProcessLookupError:
            raise WorkerGone("exited-reaped-mid-verify") from error
        head, tail = raw.rsplit(b")", 1)
        fields = tail.split()
        if (
            int(head.split(b"(", 1)[0]) != pid
            or int(fields[1]) != ppid
            or int(fields[2]) != pgid
        ):
            raise ValueError("startup process ancestry mismatch")
        return self.completed(pair, raw, head, fields, pid, ppid, pgid, pin, parent)

    def settle_published(self, pair, worker, dev):
        """The holder found a live-verified worker ended before retention."""
        return self.exited(pair, worker["pid"], worker["ppid"], worker["pgid"], self.config["child"],
                           dev, ValueError("startup worker exited before retention"))

    def completed(self, pair, raw, head, fields, pid, ppid, pgid, pin, parent):
        """A compiler worker already exited at first sight: identity from stat only.

        A zombie has no readable cmdline, cwd or exe. Within zfb only
        WorkerLauncher spawns the zfb image with process_group(0) (parent.rs:236-245),
        so the private group, the exact zfb parent birth, the image name as comm,
        a start no earlier than zfb's and a zero exit status bind it.
        """
        process, directory = pair
        comm = head.split(b"(", 1)[1]
        if (
            fields[0] != b"Z"
            or pgid != pid
            or ppid != parent["pid"]
            or comm != os.path.basename(pin["path"]).encode()[:15]
            or int(fields[19]) < int(parent["startTicks"])
        ):
            raise ValueError("startup role first seen terminal")
        # Without ptrace read access the kernel prints 0 for exit_code.
        if os.fstat(directory).st_uid != os.geteuid():
            raise ValueError("startup transient exit status unreadable")
        if len(fields) <= STAT_EXIT_CODE:
            raise ValueError("startup transient exit status unreadable")
        exit_code = int(fields[STAT_EXIT_CODE])
        if exit_code != 0:
            raise ValueError("startup transient compiler exited unsuccessfully")
        self.paired(process, pid, True)
        poller = select.poll()
        poller.register(process, select.POLLIN)
        if not poller.poll(0):
            raise ValueError("startup transient pidfd not terminal")
        return dict(
            pid=pid,
            ppid=ppid,
            pgid=pgid,
            bootId=self.seed["bootId"],
            startTicks=fields[19].decode(),
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
                worker = transient_parent is not None
                try:
                    process = os.pidfd_open(pid, 0)
                except ProcessLookupError:
                    if worker:
                        raise WorkerGone("pidfd-open-esrch")
                    raise
                cleanup.callback(os.close, process)
                try:
                    directory = os.open(f"/proc/{pid}", os.O_RDONLY | os.O_DIRECTORY)
                except FileNotFoundError:
                    if worker:
                        raise WorkerGone("procdir-enoent")
                    raise
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
            self.trace.append(dict(sweep=self.sweeps, ns=str(time.monotonic_ns()),
                                   nativeCminflt=vb["cminflt"], children=len(workers)))
            if not workers:
                # DESIGN: no compiler child is listed. Before zfb's first reap its
                # cminflt is 0; after it, the config worker has been waited for.
                if vb["cminflt"] > 0:
                    self.conclude(rb, ri, vb, vi)
                return
            if len(workers) != 1:
                raise ValueError("ambiguous startup compiler child")
            pid = workers.pop()
            try:
                w, wb, wi = bind(
                    pid,
                    vb["pid"],
                    pid,
                    fixed["child"],
                    [fixed["child"]["path"], COMPILER_WORKER_ARG],
                    vb,
                )
            except WorkerGone as gone:
                # Reaped between children() and binding: never observed. The next
                # sweep decides on the native evidence; keep the partial attribution.
                self.reaped_mid_bind.append(dict(sweep=self.sweeps, pid=pid, reason=str(gone)))
                return
            roles = [(rb, ri, None), (vb, vi, None)]
            if "completedTransient" in wb:
                # The pidfd is bound to that exact exited process; zfb may reap it
                # at any moment, so its procdir is not read again.
                poller = select.poll()
                poller.register(w[0], select.POLLIN)
                if not poller.poll(0):
                    raise ValueError("startup role changed during verification")
            else:
                roles.append((wb, wi, vb))
            try:
                for prior, inputs, parent in roles:
                    after = self.verify(*inputs, parent)
                    if any(
                        prior[k] != after[k]
                        for k in ("pid", "ppid", "pgid", "bootId", "startTicks")
                    ):
                        raise ValueError("startup role changed during verification")
                    if parent is not None and "completedTransient" in after:
                        wb = after  # exited after binding: zombie custody
                if time.monotonic_ns() >= self.end_ns or self.stop.is_set():
                    raise ValueError("startup live handoff expired")
                self.publish(w, wb, vb, rb)
            except WorkerGone as gone:
                self.reaped_mid_bind.append(dict(sweep=self.sweeps, pid=pid, reason=str(gone)))
                return
            self.status = "retained"

    def conclude(self, rb, ri, vb, vi):
        """Worker not observed: zfb's own wait on a child is the only remaining evidence."""
        for prior, inputs in ((rb, ri), (vb, vi)):
            after = self.verify(*inputs)
            if any(prior[k] != after[k] for k in ("pid", "ppid", "pgid", "bootId", "startTicks")):
                raise ValueError("startup role changed during verification")
        if after["cminflt"] < vb["cminflt"] or self.children(inputs[0][1]):
            raise ValueError("startup role changed during verification")
        if time.monotonic_ns() >= self.end_ns or self.stop.is_set():
            raise ValueError("startup live handoff expired")
        zero = [t for t in self.trace if t["nativeCminflt"] == 0 and t["children"] == 0]
        lower = (dict(kind="observed-zero", **zero[-1]) if zero
                 else dict(kind="native-birth", startTicks=vb["startTicks"], nativeCminflt=0))
        record = dict(
            outcome="product-completion",
            root={k: rb[k] for k in ("pid", "ppid", "pgid", "bootId", "startTicks", "birthVersion")},
            native={k: vb[k] for k in ("pid", "ppid", "pgid", "bootId", "startTicks", "birthVersion")},
            bracket=dict(lower=lower, upper=dict(self.trace[-1], nativeCminflt=after["cminflt"])),
            trace=self.trace[-TRACE_CAP:],
            traceLength=len(self.trace),
            reapedMidBind=self.reaped_mid_bind[-TRACE_CAP:],
            observedNs=str(time.monotonic_ns()),
        )
        if self.complete is None:
            raise ValueError("startup completion has no publisher")
        self.complete(record)
        self.status = "worker-not-observed"

    def run(self):
        try:
            while (
                not self.stop.is_set()
                and self.sweeps < 150
                and time.monotonic_ns() < self.end_ns
            ):
                self.sweeps += 1
                self.sweep()
                if self.status in ("retained", "worker-not-observed"):
                    break
                self.stop.wait(0.020)
            if self.status not in ("retained", "worker-not-observed"):
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
