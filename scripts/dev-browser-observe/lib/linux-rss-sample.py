"""One bounded, read-only Linux renderer RSS sample. Stdout is a JSONL journal."""

import time

# First script observation. Interpreter startup and importing time precede it.
ENTRY_NS = time.monotonic_ns()

import contextlib
import ctypes
import errno
import json
import os
import re
import resource
import select
import signal
import sys

MAX_OWNED = 64
MAX_TABLE = 4096
MAX_TASKS = 256
MAX_PROC_INPUT = 16 << 20
MAX_READS = 32768
MAX_STATUS = 64 << 10
MAX_STAT = 8192
MAX_ROLLUP = 16 << 10
MAX_REQUEST = 64 << 10
MAX_LINE = 128 << 10
MAX_OUTPUT = 512 << 10
FD_LIMIT = 160
DIR = os.O_DIRECTORY | os.O_RDONLY | os.O_CLOEXEC
GONE = (FileNotFoundError, ProcessLookupError)
STATUS = ("VmRSS", "RssAnon", "RssFile", "RssShmem", "VmSwap")
ROLLUP = ("Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb", "Swap")
CLOCK = time.monotonic_ns
TIMING_VERSION = "linux-rss-timing.v1"


class Invalid(Exception):
    def __init__(self, reason, observed=None):
        super().__init__(reason)
        self.observed = observed


def stat_row(raw, pid):
    head, sep, tail = raw.rpartition(b")")
    number, delim, name = head.partition(b"(")
    fields = tail.split()
    if not raw.endswith(b"\n"):
        raise Invalid("truncated-proc-stat")
    if not (sep and delim and number.strip() == str(pid).encode() and len(fields) >= 20
            and fields[0] in b"R S D Z T t X x K W P I".split()
            and all(fields[i].isdigit() for i in (1, 2, 19))):
        raise Invalid("malformed-proc-stat")
    return dict(pid=pid, ppid=int(fields[1]), pgid=int(fields[2]),
                state=fields[0].decode(), comm=name.decode(errors="replace"),
                startTicks=fields[19].decode())


def counters(raw, names):
    if not raw.endswith(b"\n"):
        raise Invalid("truncated-proc-counters")
    result = dict.fromkeys(names)
    for line in raw.splitlines():
        key, sep, value = line.partition(b":")
        name = key.decode(errors="replace")
        if name not in result or not sep:
            continue
        value = re.fullmatch(rb"\s*(\d+)\s+kB\s*", value)
        if value is None or result[name] is not None or int(value[1]) > 2**53-1:
            raise Invalid("malformed-proc-counter", {"field": name})
        result[name] = int(value[1])
    return result


def owned(rows, root):
    """Same PGID + transitive PPID selection as process-owner.mjs/ownedProcesses."""
    selected = {p["pid"]: p for p in rows if p["pgid"] == root["pgid"]}
    while True:
        size = len(selected)
        for p in rows:
            if p["ppid"] == root["pid"] or p["ppid"] in selected:
                selected[p["pid"]] = p
        if len(selected) == size:
            return selected


def birth_matches(a, b):
    return all(a[k] == b[k] for k in ("pid", "pgid", "bootId", "startTicks"))


def terminal(fd):
    poll = select.poll()
    poll.register(fd, select.POLLIN)
    events = poll.poll(0)
    if not events:
        return None
    mask = events[0][1]
    if not mask & (select.POLLIN | select.POLLHUP) or mask & ~(select.POLLIN | select.POLLHUP):
        raise Invalid("unexpected-pidfd-poll", {"mask": mask})
    return dict(pidfdFlags=0, pollMask=mask, observedNs=str(CLOCK()))


class Sample:
    def __init__(self, request, emit, timing=lambda *args: None):
        self.root = request["root"]
        self.known = {p["pid"]: p for p in request["known"]}
        self.proofs = {p["birth"]["pid"]: p for p in request["terminalProofs"]}
        self.custody = None
        self.exe = request["executable"]
        self.fixed_native = None
        self.fixed_child = request.get("fixedChildExecutable")
        self.fixed_dev_args = request.get("fixedDevArgs")
        self.fixed_workers = self.fixed_dev_args is not None
        self.fixed_launch = request.get("fixedLaunch")
        self.emit = emit
        self.timing = timing
        self.bytes = self.reads = 0
        self.stage = "discovery"
        self.expected = self.root
        self.observed = self.proof = self.partial = None
        self.started = str(CLOCK())

    def read(self, path, cap, directory=None):
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_CLOEXEC, dir_fd=directory), "rb",
                       buffering=0) as f:
            chunks = bytearray()
            while len(chunks) <= cap:
                self.reads += 1
                if self.reads > MAX_READS:
                    raise Invalid("proc-read-cap")
                data = f.read(min(65536, cap + 1 - len(chunks)))
                self.bytes += len(data)
                if self.bytes > MAX_PROC_INPUT:
                    raise Invalid("proc-input-cap")
                if not data:
                    return bytes(chunks)
                chunks.extend(data)
            raise Invalid("proc-file-cap", {"file": path, "limit": cap})

    def rows(self, directory, cap):
        with os.scandir(directory) as entries:
            for count, e in enumerate(entries, 1):
                if count > cap:
                    raise Invalid("directory-entry-cap")
                if e.name.isdecimal() and len(e.name) <= 10:
                    yield int(e.name)

    def boot(self):
        boot = self.read("/proc/sys/kernel/random/boot_id", 64).strip().decode("ascii")
        if boot != self.root["bootId"]:
            raise Invalid("boot-mismatch", {"bootId": boot})
        return boot

    def stat(self, pid, directory, boot):
        return dict(stat_row(self.read("stat", MAX_STAT, directory), pid),
                    bootId=boot, birthVersion="linux-proc-v1")

    def mark(self, stage, expected, observed=None):
        self.stage, self.expected, self.observed = stage, expected, observed
        self.proof = None

    def bind(self, stack, candidate):
        pid = candidate["pid"]
        self.mark("binding", candidate)
        fd = os.pidfd_open(pid, 0)
        stack.callback(os.close, fd)
        directory = os.open(f"/proc/{pid}", DIR)
        stack.callback(os.close, directory)
        row = self.stat(pid, directory, self.boot())
        self.observed = row
        previous = self.known.get(pid)
        if not birth_matches(candidate, row) or (previous and not birth_matches(previous, row)):
            raise Invalid("birth-or-group-mismatch", row)
        # fdinfo still names this PID only while the process referred to by the
        # pidfd has not been reaped. Thus the proc directory we already hold is
        # paired with the pidfd. Neither descriptor can redirect later reads.
        info = self.read(f"/proc/self/fdinfo/{fd}", MAX_STAT)
        if re.findall(rb"(?m)^Pid:\s*(-?\d+)$", info) != [str(pid).encode()]:
            raise Invalid("unbound-pidfd-proc-directory", row)
        return row, fd, directory

    def check_root(self, bound):
        row, fd, directory = bound
        self.mark("root-check", self.root)
        got = self.stat(row["pid"], directory, self.boot())
        self.observed = got
        if not birth_matches(self.root, got):
            raise Invalid("root-lost-or-unverified", got)
        ended = terminal(fd)
        self.proof = dict(ended, birth=row) if ended else None
        if ended or got["state"] == "Z":
            raise Invalid("root-lost-or-unverified", got)
        return got

    @staticmethod
    def argv_bytes(argv):
        return b"".join(os.fsencode(arg) + b"\0" for arg in argv)

    def exe_identity(self, directory, row=None):
        expected = self.exe
        if self.fixed_child and row["pid"] != self.root["pid"]:
            if not self.fixed_workers and row["ppid"] != self.root["pid"]:
                raise Invalid("unexpected-fixed-process-role", row)
            expected = self.fixed_child
        path = os.readlink("exe", dir_fd=directory)
        if path != expected["path"]:
            raise Invalid("unexpected-executable", {"path": path})
        info = os.stat("exe", dir_fd=directory)
        exe = dict(path=path, device=str(info.st_dev), inode=str(info.st_ino))
        if exe != expected:
            raise Invalid("unexpected-executable", exe)
        if self.fixed_workers and row["pid"] == self.root["pid"]:
            launch = self.fixed_launch
            if not launch or os.readlink("cwd", dir_fd=directory) != launch["cwd"]:
                raise Invalid("unexpected-fixed-root-cwd", row)
            shim = launch["shim"]
            info = os.stat(shim["path"])
            if (str(info.st_dev), str(info.st_ino)) != (shim["device"], shim["inode"]):
                raise Invalid("unexpected-fixed-shim-identity", row)
            argv = self.read("cmdline", MAX_STAT, directory)
            if argv != self.argv_bytes([path, shim["path"], *self.fixed_dev_args]):
                raise Invalid("unexpected-fixed-root-argv", row)
        if self.fixed_workers and row["pid"] != self.root["pid"]:
            if row["ppid"] == self.root["pid"]:
                if row["pgid"] != self.root["pgid"]:
                    raise Invalid("unexpected-fixed-native-group", row)
                if self.fixed_native and not birth_matches(self.fixed_native, row):
                    raise Invalid("ambiguous-fixed-native-child", row)
                expected_argv = [path, *self.fixed_dev_args]
                if self.read("cmdline", MAX_STAT, directory) != self.argv_bytes(expected_argv):
                    raise Invalid("unexpected-fixed-dev-argv", row)
                self.fixed_native = row
            else:
                if (self.fixed_native is None or row["ppid"] != self.fixed_native["pid"]
                        or row["pgid"] != row["pid"]):
                    raise Invalid("unexpected-fixed-process-role", row)
                argv = self.read("cmdline", MAX_STAT, directory)
                if argv != self.argv_bytes((path, "__zfb-compiler-worker")):
                    raise Invalid("unexpected-fixed-compiler-argv", row)
                exe["workerArg"] = "__zfb-compiler-worker"
        return exe

    def threads(self, directory, boot):
        taskfd = os.open("task", DIR, dir_fd=directory)
        try:
            hosts, listed, vanished = [], 0, 0
            for tid in self.rows(taskfd, MAX_TASKS):
                listed += 1
                try:
                    row = stat_row(self.read(f"{tid}/stat", MAX_STAT, taskfd), tid)
                except GONE:
                    vanished += 1
                    continue
                if row["comm"] == "zfb-v8-host":
                    hosts.append(dict(tid=tid, bootId=boot, comm=row["comm"],
                                      startTicks=row["startTicks"]))
            return dict(listed=listed, vanished=vanished,
                        rendererHosts=sorted(hosts, key=lambda r: r["tid"]))
        finally:
            os.close(taskfd)

    def observe(self, binding):
        row, fd, directory = binding
        self.mark("metadata", row, row)
        pid = row["pid"]
        result = self.partial = dict(row, executable=None, statusKiB=None, smapsRollupKiB=None,
                      smapsRollupUnavailable=None, rssKiB=None, threads=None,
                      startedNs=str(CLOCK()))
        ended = terminal(fd)
        if not ended:
            try:
                result["executable"] = self.exe_identity(directory, row)
                self.stage = "status"
                def memory(path, cap, names, field, source):
                    raw = self.read(path, cap, directory)
                    # Validate uniqueness and framing of the safety value first.
                    # An unrelated malformed field must still fail the sample,
                    # but cannot erase this sound RSS observation.
                    value = counters(raw, (field,))[field]
                    if value is not None:
                        self.emit({"type": "rss", "birth": row, "source": source,
                                   "rssKiB": value, "observedNs": str(CLOCK())})
                        result["rssKiB"] = max(result["rssKiB"] or 0, value)
                    return counters(raw, names)
                result["statusKiB"] = memory("status", MAX_STATUS, STATUS, "VmRSS", "VmRSS")
                self.stage = "rollup"
                rollup = memory("smaps_rollup", MAX_ROLLUP, ROLLUP, "Rss", "rollup.Rss")
                if not all(rollup[k] is not None for k in ("Rss", "Private_Clean", "Private_Dirty", "Swap")):
                    raise Invalid("incomplete-rollup", rollup)
                result["smapsRollupKiB"] = rollup
                self.stage = "threads"
                result["threads"] = self.threads(directory, row["bootId"])
                self.stage = "final-executable"
                self.exe_identity(directory, row)
            except GONE as error:
                ended = terminal(fd)
                if not ended:
                    raise
                result["interruptedRead"] = {"stage": self.stage, "errno": error.errno}
        self.stage = "final-identity"
        try:
            final = self.stat(pid, directory, self.boot())
            self.observed = final
            if self.fixed_workers and row["pid"] != self.root["pid"] and final["ppid"] != row["ppid"]:
                raise Invalid("fixed-parent-changed", final)
            if not birth_matches(row, final):
                raise Invalid("birth-or-group-mismatch", final)
        except GONE:
            if not terminal(fd):
                raise
        ended = terminal(fd) or ended
        if ended:
            proof = dict(ended, birth=row)
            self.proof = proof
            if pid == self.root["pid"]:
                raise Invalid("root-terminated", row)
            result.update(disposition="proved-departure", terminalProof=proof)
            if self.fixed_workers and not result["executable"]:
                result["fixedRole"] = "unknown"
        elif (result["statusKiB"]["VmRSS"] is None or not result["threads"]["listed"]
              or final["state"] == "Z"):
            raise Invalid("live-metadata-unavailable", self.observed)
        else:
            result["disposition"] = "live"
            if self.custody and result["executable"].get("workerArg") == "__zfb-compiler-worker":
                self.stage = "compiler-custody"
                result["custodyToken"] = self.custody.retain(row, binding, self.fixed_native)
        result["endNs"] = str(CLOCK())
        self.emit({"type": "observation", "process": result})

    def verify_chain(self, row, bindings):
        seen = set()
        while row["pgid"] != self.root["pgid"]:
            parent = row["ppid"]
            if parent in seen or parent not in bindings:
                raise Invalid("unbound-ancestry", row)
            seen.add(parent)
            expected, fd, directory = bindings[parent]
            self.mark("parent-check", expected)
            row = self.stat(parent, directory, self.boot())
            self.observed = row
            if not birth_matches(expected, row) or terminal(fd):
                raise Invalid("unverified-ancestry", row)
        self.check_root(bindings[self.root["pid"]])

    def handoff(self, row):
        self.emit({"type": "bound", "birth": row, "binding": {
            "pidfdFlags": 0, "fdinfoPid": row["pid"], "verifiedNs": str(CLOCK())}})

    def problem(self, exc):
        prepared = CLOCK()
        observed = getattr(exc, "observed", None)
        result = dict(type="failure", _preparedNs=str(prepared), stage=self.stage, error=str(exc)[:256],
                    errno=getattr(exc, "errno", None),
                    syscallError=errno.errorcode.get(getattr(exc, "errno", None)),
                    expected=self.expected, observed=observed if observed is not None else self.observed,
                    terminalProof=self.proof, partialProcess=self.partial,
                    startedNs=self.started, endNs=str(CLOCK()),
                    readCalls=self.reads, procInputBytes=self.bytes,
                    helperMaxRssKiB=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)

        result["_preparedEndNs"] = str(CLOCK())
        return result

    @contextlib.contextmanager
    def descriptors(self):
        stack = contextlib.ExitStack()
        try:
            yield stack
        finally:
            # Even a broken output pipe cannot skip descriptor cleanup.
            try:
                self.timing("cleanupBegin", [CLOCK()])
            finally:
                started = CLOCK()
                try:
                    stack.close()
                finally:
                    self.timing("fdCleanup", [started, CLOCK()])

    def run(self):
        if len(self.known) > MAX_OWNED or self.root["pgid"] != self.root["pid"]:
            raise Invalid("invalid-root-or-known-cap")
        with self.descriptors() as stack:
            root = self.bind(stack, self.root)
            self.check_root(root)
            self.handoff(root[0])
            boot = self.boot()
            self.mark("discovery", self.root)
            table = {}
            bindings = {self.root["pid"]: root}
            claimed = {self.root["pid"]}
            first_failure = None
            attempted = set()

            def bind_candidate(candidate):
                nonlocal first_failure
                attempted.add(candidate["pid"])
                try:
                    binding = self.bind(stack, candidate)
                    row, fd, _ = binding
                    pid = row["pid"]
                    if (pid not in self.known and row["pgid"] != self.root["pgid"]
                            and row["ppid"] != candidate["ppid"]):
                        raise Invalid("handoff-ancestry-changed", row)
                    reachable = row["pgid"] == self.root["pgid"] or row["ppid"] in claimed
                    if not reachable and pid not in self.known:
                        raise Invalid("unbound-ancestry", row)
                    if reachable:
                        self.verify_chain(row, bindings)
                    else:
                        self.check_root(root)
                    self.mark("handoff", row, row)
                    bindings[pid] = binding
                    if reachable:
                        claimed.add(pid)
                    self.handoff(row)
                    if pid in self.proofs and not terminal(fd):
                        self.proof = self.proofs[pid]
                        raise Invalid("contradictory-terminal-proof", row)
                except (OSError, ValueError, Invalid) as exc:
                    if isinstance(exc, GONE) and self.stage == "binding":
                        error = Invalid("unbound-candidate-departure", {"errno": exc.errno})
                        error.errno = exc.errno
                        exc = error
                    first_failure = first_failure or self.problem(exc)

            for pid in self.rows("/proc", MAX_TABLE):
                try:
                    row = stat_row(self.read(f"/proc/{pid}/stat", MAX_STAT), pid)
                except GONE:
                    # A directory name alone is not a selection or verified birth.
                    continue
                table[pid] = dict(row, bootId=boot, birthVersion="linux-proc-v1")
                # A fixed startup worker can exit while unrelated /proc rows are
                # scanned. Bind only after a parent or the owned group is known.
                candidate = table[pid]
                if (self.fixed_workers and pid != self.root["pid"]
                        and (candidate["pgid"] == self.root["pgid"] or candidate["ppid"] in claimed)):
                    if len(attempted | self.known.keys() | {pid, self.root["pid"]}) > MAX_OWNED:
                        raise Invalid("owned-process-cap")
                    bind_candidate(candidate)
                    self.mark("discovery", self.root)
            candidates = owned(table.values(), self.root)
            for pid, previous in self.known.items():
                if pid in table:
                    # A known birth is mandatory even after a change of ancestry.
                    # bind() compares this candidate with that immutable birth.
                    candidates[pid] = table[pid]
                elif pid in self.proofs and pid != self.root["pid"]:
                    self.emit({"type": "observation", "process": dict(previous,
                               disposition="retained-terminal-proof", terminalProof=self.proofs[pid],
                               rssKiB=None, statusKiB=None, smapsRollupKiB=None, threads=None,
                               executable=None, startedNs=self.started, endNs=str(CLOCK()))})
                elif self.custody and str(pid) in self.custody.tokens:
                    self.mark("custody-known", previous)
                    with contextlib.ExitStack() as duplicates:
                        fd, directory = self.custody.retrieve(previous)
                        duplicates.callback(os.close, fd)
                        duplicates.callback(os.close, directory)
                        self.boot()
                        ended = terminal(fd)
                        if not ended:
                            raise Invalid("custodied-known-missing-without-terminal")
                        self.emit({"type":"observation","process":dict(previous,
                                  disposition="custodied-departure", terminalProof=dict(ended,birth=previous),
                                  custodyToken=self.custody.tokens[str(pid)], rssKiB=None, statusKiB=None,
                                  smapsRollupKiB=None, threads=None, executable=None,
                                  startedNs=self.started,endNs=str(CLOCK()))})
                else:
                    self.mark("discovery-known", previous)
                    first_failure = first_failure or self.problem(Invalid("unbound-known-departure"))
            if len(candidates.keys() | self.known.keys()) > MAX_OWNED:
                raise Invalid("owned-process-cap")
            # Walk parent-before-child. Independent branches can still hand off
            # their verified births if a different branch fails before binding.
            pending = [row for pid, row in candidates.items() if pid not in claimed and pid not in attempted]
            while pending:
                deferred = []
                for candidate in pending:
                    if (candidate["pgid"] != self.root["pgid"] and candidate["ppid"] not in claimed
                            and candidate["pid"] not in self.known):
                        deferred.append(candidate)
                        continue
                    bind_candidate(candidate)
                if len(deferred) == len(pending):
                    self.mark("ancestry", deferred[0])
                    first_failure = first_failure or self.problem(Invalid("unbound-ancestry"))
                    break
                pending = deferred
            if first_failure:
                self.emit(first_failure)
                return False
            # Role checks need the live direct native parent's identity first.
            ordered = bindings.values()
            if self.fixed_workers:
                ordered = sorted(ordered, key=lambda item: item[0]["ppid"] != self.root["pid"])
            for bound in ordered:
                try:
                    if (self.fixed_workers and bound != root and bound[0]["ppid"] != self.root["pid"]
                            and not terminal(bound[1])):
                        self.verify_chain(bound[0], bindings)
                    self.observe(bound)
                except (OSError, ValueError, Invalid) as exc:
                    # Include any available terminal evidence alongside the
                    # failure, without overriding contradictory observations.
                    try:
                        proof = terminal(bound[1])
                        self.proof = dict(proof, birth=bound[0]) if proof else None
                    except (OSError, Invalid):
                        self.proof = None
                    self.emit(self.problem(exc))
                    return False
                self.partial = None
            self.timing("finalRootBegin", [CLOCK()])
            started = CLOCK()
            try:
                self.check_root(root)
            finally:
                self.timing("finalRoot", [started, CLOCK()])
        prepared = CLOCK()
        self.emit(dict(type="complete", _preparedNs=str(prepared), readCalls=self.reads, procInputBytes=self.bytes,
                    helperMaxRssKiB=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss))
        return True


class Journal:
    """Cap timing bytes and semantic bytes together. No frame times its own flush."""
    def __init__(self, stream):
        self.stream = stream
        self.bytes = self.sequence = 0

    def write(self, row):
        start = CLOCK()
        line = json.dumps(row, separators=(",", ":"), ensure_ascii=True).encode() + b"\n"
        serialized = CLOCK()
        self.bytes += len(line)
        if len(line) > MAX_LINE or self.bytes > MAX_OUTPUT:
            raise Invalid("output-cap")
        self.stream.write(line)
        written = CLOCK()
        self.stream.flush()
        return [start, serialized, written, CLOCK()]

    def timing(self, stage, ns, **fields):
        self.write(dict(type="timing", version=TIMING_VERSION, seq=self.sequence,
                        stage=stage, ns=[str(n) for n in ns], **fields))
        self.sequence += 1

    def emit(self, row):
        kind = row["type"]
        prepared = row.pop("_preparedNs", None)
        prepared_end = row.pop("_preparedEndNs", None)
        if prepared is not None and prepared_end is None:
            prepared_end = CLOCK()
        spans = self.write(row)
        if kind == "observation":
            self.timing("observationOutput", spans, pid=row["process"]["pid"])
        elif kind in ("complete", "failure"):
            self.timing("terminalOutput", [prepared, prepared_end, *spans])


def main():
    # Linux PR_SET_PDEATHSIG. Check the parent again after registration to
    # cover controller death between spawn and setting the safeguard.
    if ctypes.CDLL(None, use_errno=True).prctl(1, signal.SIGKILL, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_SET_PDEATHSIG")
    if os.getppid() != int(sys.argv[1]):
        raise Invalid("sampling-controller-exited")
    soft_as, hard_as = resource.getrlimit(resource.RLIMIT_AS)
    cap = 96 << 20
    resource.setrlimit(resource.RLIMIT_AS, (cap if soft_as == -1 else min(soft_as, cap), hard_as))
    # Two retained fds per selected process, with margin for transient reads.
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (FD_LIMIT if soft == -1 else min(soft, FD_LIMIT), hard))
    read_started = CLOCK()
    raw = sys.stdin.buffer.read(MAX_REQUEST + 1)
    read_ended = CLOCK()
    if len(raw) > MAX_REQUEST:
        raise Invalid("request-byte-cap")
    request = json.loads(raw)
    decoded = CLOCK()
    journal = Journal(sys.stdout.buffer)
    emit = journal.emit
    sample = Sample(dict(request, executable=None), emit, journal.timing)
    try:
        sample.mark("helper-start", sample.root)
        pid = os.getpid()
        try:
            namespace = os.readlink("/proc/self/ns/time")
        except OSError:
            namespace = None
        try:
            boot = sample.read("/proc/sys/kernel/random/boot_id", 64).strip().decode("ascii")
        except (OSError, ValueError, Invalid):
            boot = None
        journal.timing("request", [ENTRY_NS, read_started, read_ended, decoded],
                       clock=dict(implementation=time.get_clock_info("monotonic").implementation,
                                  bootId=boot, timeNamespace=namespace))
        birth = dict(stat_row(sample.read("/proc/self/stat", MAX_STAT), pid),
                     bootId=sample.boot(), birthVersion="linux-proc-v1")
        emit(dict(type="helper", birth=birth))
        sample.stage = "expected-executable"
        path = os.path.realpath(request["executablePath"], strict=True)
        stat = os.stat(path)
        sample.exe = dict(path=path, device=str(stat.st_dev), inode=str(stat.st_ino))
        emit(dict(type="executable", pathInput=request["executablePath"], executable=sample.exe))
        with contextlib.ExitStack() as resources:
            if "fixedCustody" in request:
                # Isolated Python (-I) does not search the script's directory.
                import importlib.util
                spec = importlib.util.spec_from_file_location("fixed_custody",
                       os.path.join(os.path.dirname(__file__), "linux-rss-custody.py"))
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                sample.custody = module.Client(request["fixedCustody"])
                resources.callback(sample.custody.close)
                for pid, proof in sample.proofs.items():
                    token = sample.custody.tokens.get(str(pid))
                    if token:
                        sample.custody.release(proof["birth"], token)
                        emit(dict(type="custody-release", birth=proof["birth"], token=token))
            code = 0 if sample.run() else 1
    except (OSError, ValueError, Invalid) as exc:
        emit(sample.problem(exc))
        code = 1
    journal.timing("preExit", [CLOCK()])
    return code


if __name__ == "__main__":
    sys.exit(main())
