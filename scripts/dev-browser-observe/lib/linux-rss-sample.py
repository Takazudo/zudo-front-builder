"""One bounded, read-only Linux renderer RSS sample. Stdout is a JSONL journal."""

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
import time

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
    def __init__(self, request, emit):
        self.root = request["root"]
        self.known = {p["pid"]: p for p in request["known"]}
        self.proofs = {p["birth"]["pid"]: p for p in request["terminalProofs"]}
        self.exe = request["executable"]
        self.emit = emit
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

    def exe_identity(self, directory):
        path = os.readlink("exe", dir_fd=directory)
        if path != self.exe["path"]:
            raise Invalid("unexpected-executable", {"path": path})
        info = os.stat("exe", dir_fd=directory)
        exe = dict(path=path, device=str(info.st_dev), inode=str(info.st_ino))
        if exe != self.exe:
            raise Invalid("unexpected-executable", exe)
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
                result["executable"] = self.exe_identity(directory)
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
                self.exe_identity(directory)
            except GONE as error:
                ended = terminal(fd)
                if not ended:
                    raise
                result["interruptedRead"] = {"stage": self.stage, "errno": error.errno}
        self.stage = "final-identity"
        try:
            final = self.stat(pid, directory, self.boot())
            self.observed = final
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
        elif (result["statusKiB"]["VmRSS"] is None or not result["threads"]["listed"]
              or final["state"] == "Z"):
            raise Invalid("live-metadata-unavailable", self.observed)
        else:
            result["disposition"] = "live"
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
        observed = getattr(exc, "observed", None)
        return dict(type="failure", stage=self.stage, error=str(exc)[:256],
                    errno=getattr(exc, "errno", None),
                    syscallError=errno.errorcode.get(getattr(exc, "errno", None)),
                    expected=self.expected, observed=observed if observed is not None else self.observed,
                    terminalProof=self.proof, partialProcess=self.partial,
                    startedNs=self.started, endNs=str(CLOCK()),
                    readCalls=self.reads, procInputBytes=self.bytes,
                    helperMaxRssKiB=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)

    def run(self):
        if len(self.known) > MAX_OWNED or self.root["pgid"] != self.root["pid"]:
            raise Invalid("invalid-root-or-known-cap")
        with contextlib.ExitStack() as stack:
            root = self.bind(stack, self.root)
            self.check_root(root)
            self.handoff(root[0])
            boot = self.boot()
            self.mark("discovery", self.root)
            table = {}
            for pid in self.rows("/proc", MAX_TABLE):
                try:
                    row = stat_row(self.read(f"/proc/{pid}/stat", MAX_STAT), pid)
                except GONE:
                    # A directory name alone is not a selection or verified birth.
                    continue
                table[pid] = dict(row, bootId=boot, birthVersion="linux-proc-v1")
            candidates = owned(table.values(), self.root)
            first_failure = None
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
                else:
                    self.mark("discovery-known", previous)
                    first_failure = first_failure or self.problem(Invalid("unbound-known-departure"))
            if len(candidates.keys() | self.known.keys()) > MAX_OWNED:
                raise Invalid("owned-process-cap")
            bindings = {self.root["pid"]: root}
            claimed = {self.root["pid"]}
            # Walk parent-before-child. Independent branches can still hand off
            # their verified births if a different branch fails before binding.
            pending = [row for pid, row in candidates.items() if pid not in claimed]
            while pending:
                deferred = []
                for candidate in pending:
                    if (candidate["pgid"] != self.root["pgid"] and candidate["ppid"] not in claimed
                            and candidate["pid"] not in self.known):
                        deferred.append(candidate)
                        continue
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
                if len(deferred) == len(pending):
                    self.mark("ancestry", deferred[0])
                    first_failure = first_failure or self.problem(Invalid("unbound-ancestry"))
                    break
                pending = deferred
            if first_failure:
                self.emit(first_failure)
                return False
            for bound in bindings.values():
                try:
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
            self.check_root(root)
        self.emit(dict(type="complete", readCalls=self.reads, procInputBytes=self.bytes,
                    helperMaxRssKiB=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss))
        return True


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
    raw = sys.stdin.buffer.read(MAX_REQUEST + 1)
    if len(raw) > MAX_REQUEST:
        raise Invalid("request-byte-cap")
    output = 0

    def emit(row):
        nonlocal output
        line = json.dumps(row, separators=(",", ":"), ensure_ascii=True).encode() + b"\n"
        output += len(line)
        if len(line) > MAX_LINE or output > MAX_OUTPUT:
            raise Invalid("output-cap")
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()

    request = json.loads(raw)
    sample = Sample(dict(request, executable=None), emit)
    try:
        sample.mark("helper-start", sample.root)
        pid = os.getpid()
        birth = dict(stat_row(sample.read("/proc/self/stat", MAX_STAT), pid),
                     bootId=sample.boot(), birthVersion="linux-proc-v1")
        emit(dict(type="helper", birth=birth))
        sample.stage = "expected-executable"
        path = os.path.realpath(request["executablePath"], strict=True)
        stat = os.stat(path)
        sample.exe = dict(path=path, device=str(stat.st_dev), inode=str(stat.st_ino))
        emit(dict(type="executable", pathInput=request["executablePath"], executable=sample.exe))
        return 0 if sample.run() else 1
    except (OSError, ValueError, Invalid) as exc:
        emit(sample.problem(exc))
        return 1


if __name__ == "__main__":
    sys.exit(main())
