"""Bounded Next identity scout. No RSS reads. Transfer only the original bound descriptor pair."""

import array
import ctypes
import hashlib
import json
import os
from pathlib import Path
import resource
import secrets
import select
import signal
import socket
import stat
import struct
import sys
import time

MAX_PACKET = 32768
MAX_FDS = 2
U32 = struct.Struct("3i")
CLOEXEC = socket.MSG_CMSG_CLOEXEC
now = time.monotonic_ns
boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
cfg = {}
proc_bytes = 0
proc_reads = 0


def require(test, message):
    if not test:
        raise ValueError(message)


def send(sock, data, fds=()):
    payload = json.dumps(data, separators=(",", ":")).encode()
    require(len(payload) <= MAX_PACKET and len(fds) <= MAX_FDS, "custody message cap")
    ancillary = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds))] if fds else []
    require(sock.sendmsg([payload], ancillary) == len(payload), "incomplete custody send")


def receive(sock, want, credential=False):
    cap = socket.CMSG_SPACE(MAX_FDS * 4) + socket.CMSG_SPACE(U32.size)
    payload, ancillary, flags, _ = sock.recvmsg(MAX_PACKET, cap, CLOEXEC)
    fds = []
    creds = []
    valid = True
    try:
        for level, kind, value in ancillary:
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                incoming = array.array("i")
                incoming.frombytes(value)
                fds.extend(incoming)
            elif level == socket.SOL_SOCKET and kind == socket.SCM_CREDENTIALS:
                creds.append(U32.unpack(value))
            else:
                valid = False
        require(
            payload
            and not flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC)
            and valid
            and len(fds) in want
            and len(creds) == int(credential),
            "invalid custody response",
        )
        return json.loads(payload), fds, creds[0] if creds else None
    except BaseException:
        for fd in fds:
            os.close(fd)
        raise


def identity(fd):
    info = os.fstat(fd)
    return [str(info.st_dev), str(info.st_ino)]


def read_small(path, directory=None):
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC, dir_fd=directory)
    try:
        value = os.read(fd, 4097)
        require(len(value) <= 4096 and value.endswith(b"\n"), "unverified custody proc record")
        return value
    finally:
        os.close(fd)


def account_proc(data):
    global proc_bytes, proc_reads
    proc_bytes += len(data)
    proc_reads += 1
    require(proc_bytes <= 16 << 20 and proc_reads <= 131072, "scout proc input cap")


def field(fd, name, limit):
    f = os.open(name, os.O_RDONLY | os.O_CLOEXEC, dir_fd=fd)
    try:
        with os.fdopen(f, "rb", closefd=False) as reader:
            v = reader.read(limit + 1)
        account_proc(v)
        require(len(v) <= limit, "input cap " + name)
        return v
    finally:
        os.close(f)


def birth(fd):
    raw = field(fd, "stat", 4096).decode()
    pid, title = raw.split(" (", 1)
    comm, tail = title.rsplit(") ", 1)
    v = tail.split()
    return dict(
        pid=int(pid),
        birthVersion="linux-proc-v1",
        comm=comm,
        state=v[0],
        ppid=int(v[1]),
        pgid=int(v[2]),
        startTicks=v[19],
        bootId=boot,
    )


def key(b):
    return tuple(b[k] for k in ("pid", "ppid", "pgid", "startTicks", "bootId"))


def alive(b):
    require(b["state"] not in ("X", "x", "Z"), "terminal state")


def bind(pid):
    pf = os.pidfd_open(pid)
    fd = None
    try:
        fd = os.open("/proc/" + str(pid), os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        b = birth(fd)
        alive(b)
        info = Path("/proc/self/fdinfo", str(pf)).read_text().splitlines()
        require("Pid:\t" + str(pid) in info and b["pid"] == pid, "unpaired pidfd")
        return pf, fd, b
    except BaseException:
        os.close(pf)
        if fd is not None:
            os.close(fd)
        raise


def view(handle):
    _, fd, b = handle
    start = now()
    before = birth(fd)
    alive(before)
    require(key(before) == key(b), "prior birth mismatch")
    e = os.open("exe", os.O_RDONLY | os.O_CLOEXEC, dir_fd=fd)
    try:
        st = os.fstat(e)
        elf = os.read(e, 4).hex()
        image = dict(
            path=os.readlink("/proc/self/fd/" + str(e)), device=str(st.st_dev), inode=str(st.st_ino)
        )
        require(image == cfg["image"] and elf == "7f454c46", "unexpected image")
        argv = field(fd, "cmdline", 16384)
        env = field(fd, "environ", 131072).split(b"\0")
        wanted = [
            x.partition(b"=")[2].decode("ascii", "strict")
            for x in env
            if x.startswith(b"JEST_WORKER_ID=")
        ]
        require(
            len(wanted) <= 1 and all(x.isdecimal() and len(x) <= 4 for x in wanted),
            "invalid worker id",
        )
        for name in (b"NEXT_PRIVATE_WORKER", b"__NEXT_DEV_SERVER"):
            values = [value for value in env if value.startswith(name + b"=")]
            require(len(values) <= 1, "duplicate Next role environment")
        cwd = os.readlink("cwd", dir_fd=fd)
        after = birth(fd)
        alive(after)
        final = os.stat("exe", dir_fd=fd)
        require(
            key(before) == key(after) and (final.st_dev, final.st_ino) == (st.st_dev, st.st_ino),
            "unstable birth/image",
        )
        return dict(
            startNs=str(start),
            endNs=str(now()),
            birth=after,
            image=image,
            elf=elf,
            cwd=cwd,
            argvHex=argv.hex(),
            jestId=next(iter(wanted), None),
            private=b"NEXT_PRIVATE_WORKER=1" in env,
            dev=b"__NEXT_DEV_SERVER=1" in env,
        )
    finally:
        os.close(e)


def is_jest(v, c):
    args = v["argvHex"]
    return (
        v["birth"]["ppid"] == c["server"]["pid"]
        and v["birth"]["pgid"] == c["server"]["pgid"]
        and v["cwd"] == c["app"]
        and v["image"] == c["image"]
        and v["elf"] == "7f454c46"
        and "parentJestId" in c
        and v["jestId"] == c["parentJestId"]
        and v["private"] is True
        and v["dev"] is True
        and args
        == (os.fsencode(c["image"]["path"]) + b"\0" + os.fsencode(c["script"]) + b"\0").hex()
    )


def serving_socket(handle, cfg):
    fd = handle[1]
    expected = cfg["socket"]
    require(
        os.readlink("fd/" + str(expected["fd"]), dir_fd=fd) == "socket:[" + expected["inode"] + "]",
        "listener FD drift",
    )
    found = []
    for fam in ("tcp", "tcp6"):
        for line in field(fd, "net/" + fam, 2 << 20).decode().splitlines()[1:]:
            cells = line.split()
            addr, port = cells[1].split(":")
            if cells[3] == "0A" and int(port, 16) == expected["port"]:
                found.append((addr, cells[9]))
    require(found == [(expected["addr"], expected["inode"])], "listener address/inode drift")


def children(fd):
    task = os.open("task", os.O_DIRECTORY | os.O_RDONLY, dir_fd=fd)
    try:
        out = set()
        with os.scandir(task) as entries:
            for n, i in enumerate(entries):
                require(n < 128, "task cap")
                ids = field(task, i.name + "/children", 1024).split()
                out.update(map(int, ids))
                require(len(out) <= 8, "children cap")
        return out
    finally:
        os.close(task)


def poll_terminal(fd):
    poll = select.poll()
    poll.register(fd, select.POLLIN)
    result = poll.poll(0)
    if not result:
        return None
    mask = result[0][1]
    require(mask in (1, 16, 17), "unverified group pidfd state")
    return mask


def close_pair(pair):
    os.close(pair[0])
    os.close(pair[1])


def confirm_script(pin):
    fd = os.open(pin["path"], os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        require(
            stat.S_ISREG(info.st_mode)
            and str(info.st_dev) == pin["device"]
            and str(info.st_ino) == pin["inode"]
            and info.st_size == pin["size"]
            and 0 < info.st_size <= 131072,
            "bootstrap file identity changed",
        )
        content = os.read(fd, 131073)
        require(
            len(content) == info.st_size and hashlib.sha256(content).hexdigest() == pin["sha256"],
            "bootstrap file content changed",
        )
    finally:
        os.close(fd)


class Scout:
    def __init__(self, contract, session, parent):
        global cfg
        self.session = session
        self.parent = parent
        self.seq = 0
        self.input_bytes = 0
        self.output_bytes = 0
        self.known = None
        self.token = None
        self.sweeps = 0
        self.count = 0
        self.previous = None
        self.window = None
        self.end = 0
        self.next_sweep = 0
        self.anchors = []
        self.windows = []
        self.script = contract["script"]
        self.chain = contract["ancestors"]
        require(
            len(self.chain) == 3
            and contract["framework"] == "next"
            and contract["protocol"] == "linux-fixed-peer.v2",
            "unverified Next contract",
        )
        root, cli, serving = self.chain
        require(
            root["pid"] == root["pgid"]
            and root["ppid"] == parent
            and cli["ppid"] == root["pid"]
            and serving["ppid"] == cli["pid"]
            and all(b["bootId"] == boot and b["pgid"] == root["pid"] for b in self.chain),
            "unverified Next root/parent chain",
        )
        cfg = dict(
            image=contract["image"],
            app=contract["app"],
            server=serving,
            script=self.script["path"],
            parentJestId=None,
            socket=contract["socket"],
        )
        for b in self.chain:
            handle = bind(b["pid"])
            self.anchors.append(handle)
            require(key(b) == key(handle[2]), "changed Next ancestor")
        self.check_ancestors()
        self.message(
            "ready", birth=birth_of_self(), timeNamespace=os.readlink("/proc/self/ns/time")
        )

    def message(self, kind, **data):
        self.seq += 1
        packet = (
            json.dumps(
                dict(type=kind, session=self.session, seq=self.seq, ns=str(now()), **data),
                separators=(",", ":"),
            )
            + "\n"
        )
        require(
            len(packet) <= 65536 and self.output_bytes + len(packet) <= 8 << 20, "scout output cap"
        )
        self.output_bytes += len(packet)
        sys.stdout.write(packet)
        sys.stdout.flush()

    def check_ancestors(self):
        for expected, handle in zip(self.chain, self.anchors):
            v = view(handle)
            require(
                key(expected) == key(v["birth"])
                and v["cwd"] == cfg["app"]
                and v["image"] == cfg["image"],
                "ancestor identity changed",
            )
            raw = bytes.fromhex(v["argvHex"])
            match = raw == b"\0".join(os.fsencode(s) for s in expected["argv"]) + b"\0"
            if expected.get("retitled"):
                match = raw.endswith(b"\0") and raw.rstrip(b"\0") == b"next-server (v16.4.6)"
            require(match, "ancestor argv changed")
        require(v["private"] and v["dev"] and v["jestId"] is None, "serving environment changed")
        serving_socket(self.anchors[-1], cfg)

    def open(self, name, deadline):
        require(
            self.window is None
            and name not in self.windows
            and len(self.windows) < 1
            and name == "first-route",
            "invalid scout window",
        )
        start = now()
        require(start < deadline <= start + 30_000_000_000, "invalid scout window deadline")
        self.window = name
        self.end = deadline
        self.next_sweep = start
        self.sweeps = 0
        self.windows.append(name)
        self.check_ancestors()
        self.message("window-open", name=name)

    def close(self, name):
        require(
            self.window == name and (self.known is None or self.token is not None),
            "unverified worker at window close",
        )
        self.message("window-close", name=name, sweeps=self.sweeps)
        self.window = None

    def observe(self):
        target = self.next_sweep
        start = now()
        require(
            self.window and start < self.end and start - target < 50_000_000 and self.sweeps < 600,
            "identity discovery window expired or missed sweep",
        )
        self.sweeps += 1
        self.next_sweep += 50_000_000
        serving = self.anchors[-1]
        require(
            key(birth(serving[1])) == key(serving[2]) and not poll_terminal(serving[0]),
            "serving parent lost",
        )
        ids = children(serving[1])
        require(len(ids) <= 1, "ambiguous serving children")
        if self.known and poll_terminal(self.known[0]):
            require(self.token is not None, "unqualified worker departed")
            require(not ids or ids == {self.known[2]["pid"]}, "another worker before release")
            if ids:
                # Even a signalled old pidfd does not certify a new numeric birth.
                fd = os.open(
                    f"/proc/{next(iter(ids))}", os.O_DIRECTORY | os.O_RDONLY | os.O_CLOEXEC
                )
                try:
                    require(
                        key(birth(fd)) == key(self.known[2]), "reused candidate after termination"
                    )
                finally:
                    os.close(fd)
            return
        if not ids:
            require(self.known is None, "known live worker disappeared")
            return
        pid = next(iter(ids))
        if self.known is None:
            handle = bind(pid)
            try:
                self.candidate(handle[2], handle[:2])
            finally:
                close_pair(handle)
        require(pid == self.known[2]["pid"], "unexpected second worker")
        if self.token:
            require(key(birth(self.known[1])) == key(self.known[2]), "worker identity changed")
            return
        self.count += 1
        require(self.count <= 16, "identity view cap")
        checked = birth(self.known[1])
        require(
            checked["ppid"] == serving[2]["pid"]
            and checked["pgid"] == serving[2]["pgid"]
            and checked["bootId"] == boot
            and key(checked) == key(self.known[2]),
            "unverified worker ancestry",
        )
        parent_view = view(serving)
        worker_view = view(self.known)
        require(
            is_jest(worker_view, cfg)
            and parent_view["private"]
            and parent_view["dev"]
            and parent_view["jestId"] is None
            and key(birth(serving[1])) == key(serving[2]),
            "unverified generic bootstrap or parent",
        )
        confirm_script(self.script)
        evidence = dict(
            index=self.sweeps, candidateCount=1, parentView=parent_view, view=worker_view
        )
        if self.previous is not None:
            require(self.previous["index"] + 1 == self.sweeps, "nonconsecutive worker views")
            require(now() - start <= 50_000_000, "qualification sweep exceeded 50ms")
            self.token = secrets.token_hex(16)
            self.message(
                "qualified",
                birth=self.known[2],
                token=self.token,
                views=[self.previous, evidence],
                descriptors=[identity(fd) for fd in self.known[:2]],
            )
        self.previous = evidence
        require(now() - start <= 50_000_000, "identity sweep exceeded 50ms")

    def candidate(self, b, received=None):
        require(
            key(b)[1:3] == (self.chain[-1]["pid"], self.chain[0]["pgid"]) and b["bootId"] == boot,
            "candidate outside serving lineage",
        )
        if self.known:
            require(key(self.known[2]) == key(b), "another candidate or reused PID")
            if received:
                require(
                    os.readlink(f"/proc/self/fd/{received[0]}") == "anon_inode:[pidfd]"
                    and stat.S_ISDIR(os.fstat(received[1]).st_mode),
                    "wrong offered descriptor kinds",
                )
                if not poll_terminal(self.known[0]):
                    require(
                        f"Pid:\t{b['pid']}"
                        in read_small(f"/proc/self/fdinfo/{received[0]}").decode().splitlines()
                        and key(birth(received[1])) == key(b),
                        "wrong offered live pidfd or directory",
                    )
                require(
                    [identity(x) for x in received] == [identity(x) for x in self.known[:2]],
                    "different candidate descriptors",
                )
            return
        require(self.window is not None, "new worker outside discovery window")
        pair = bind(b["pid"]) if received is None else (*received, b)
        owned = received is None
        try:
            require(
                key(birth(pair[1])) == key(b) and not poll_terminal(pair[0]), "unbound candidate"
            )
            match = Path(f"/proc/self/fdinfo/{pair[0]}").read_text().splitlines()
            require(
                f"Pid:\t{b['pid']}" in match
                and os.readlink(f"/proc/self/fd/{pair[0]}") == "anon_inode:[pidfd]",
                "wrong worker pidfd",
            )
            info = os.stat(f"/proc/{b['pid']}")
            require(
                identity(pair[1]) == [str(info.st_dev), str(info.st_ino)],
                "wrong worker proc directory",
            )
            self.known = pair if owned else (os.dup(pair[0]), os.dup(pair[1]), b)
            owned = False
            self.message("bound", birth=b, descriptors=[identity(x) for x in self.known[:2]])
        finally:
            if owned:
                close_pair(pair)

    def answer(self, peer, request, helper_pid):
        require(
            self.known and self.token and key(self.known[2]) == key(request["birth"]),
            "unqualified candidate",
        )
        self.check_ancestors()
        if not poll_terminal(self.known[0]):
            confirm_script(self.script)
            require(is_jest(view(self.known), cfg), "live worker role changed after qualification")
        if request.get("token") is not None:
            require(secrets.compare_digest(request["token"], self.token), "custody token mismatch")
        self.counts["gets"] += 1
        send(
            peer,
            dict(
                session=self.session,
                cycle=request["cycle"],
                nonce=request["nonce"],
                helperPid=helper_pid,
                holderPid=os.getpid(),
                token=self.token,
                birth=self.known[2],
                descriptors=[identity(x) for x in self.known[:2]],
                acknowledgedNs=str(now()),
            ),
            self.known[:2],
        )

    def handoff(self, shared):
        size = socket.CMSG_SPACE(8) + socket.CMSG_SPACE(U32.size)
        raw, anc, flags, _ = shared.recvmsg(1, size, CLOEXEC)
        incoming = [
            fd
            for l, k, v in anc
            if (l, k) == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
            for fd in array.array("i", v)
        ]
        try:
            require(
                raw == b"H" and not flags & (socket.MSG_CTRUNC | socket.MSG_TRUNC),
                "invalid rendezvous",
            )
            credentials = [
                U32.unpack(v)
                for l, k, v in anc
                if (l, k) == (socket.SOL_SOCKET, socket.SCM_CREDENTIALS)
            ]
            require(
                len(anc) == 2 and len(credentials) == 1 and len(incoming) == 1, "rendezvous framing"
            )
            peer_pid, uid, _ = credentials[0]
            require(
                uid == os.getuid()
                and read_small(f"/proc/{peer_pid}/stat").rsplit(b")", 1)[1].split()[1]
                == str(self.parent).encode(),
                "foreign helper",
            )
            peer = socket.socket(fileno=incoming[0])
            incoming.clear()
        finally:
            for fd in incoming:
                os.close(fd)
        try:
            require(peer.type == socket.SOCK_SEQPACKET, "wrong helper socket")
            peer.setblocking(False)
            req, fds, _ = receive(peer, [0, 2])
            try:
                require(
                    req["session"] == self.session
                    and req["cycle"] > 0
                    and req["op"] in ("get", "offer")
                    and bool(fds) == (req["op"] == "offer"),
                    "wrong helper request",
                )
                cycle = req["cycle"]
                require(
                    isinstance(cycle, int)
                    and self.last_cycle <= cycle <= 240
                    and isinstance(req["nonce"], str)
                    and len(req["nonce"]) == 24,
                    "stale or invalid sampler epoch",
                )
                if cycle != self.last_cycle:
                    self.last_cycle = cycle
                    self.epoch_helper = peer_pid
                    self.outstanding_nonces.clear()
                require(
                    self.epoch_helper == peer_pid
                    and req["nonce"] not in self.outstanding_nonces
                    and len(self.outstanding_nonces) < 2,
                    "replayed or excessive helper transaction",
                )
                self.outstanding_nonces.add(req["nonce"])
                if req["op"] == "offer":
                    self.candidate(req["birth"], fds)
                require(self.known and key(self.known[2]) == key(req["birth"]), "missing custody")
                if self.token:
                    self.answer(peer, req, peer_pid)
                else:
                    require(
                        self.window and self.pending is None and req["op"] == "offer",
                        "ambiguous pending handoff",
                    )
                    self.pending = (peer, req, peer_pid)
                    peer = None
            finally:
                for fd in fds:
                    os.close(fd)
        finally:
            if peer is not None:
                peer.close()

    def loop(self, shared):
        self.pending = None
        self.counts = {"gets": 0}
        self.last_cycle = 0
        self.epoch_helper = None
        self.outstanding_nonces = set()
        shared.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        while True:
            delay = max(0, (min(self.next_sweep, self.end) - now()) / 1e9) if self.window else None
            readable, _, _ = select.select([shared, sys.stdin], [], [], delay)
            if sys.stdin in readable:
                line = sys.stdin.buffer.readline(MAX_PACKET + 1)
                require(0 < len(line) <= MAX_PACKET, "controller input lost")
                self.input_bytes += len(line)
                require(self.input_bytes <= 1 << 20, "cumulative controller input cap")
                cmd = json.loads(line)
                require(cmd["session"] == self.session, "wrong controller session")
                if cmd["op"] == "stop":
                    break
                if cmd["op"] == "open":
                    self.open(cmd["name"], int(cmd["deadline"]))
                elif cmd["op"] == "close":
                    self.close(cmd["name"])
                elif cmd["op"] == "snapshot":
                    self.candidate(cmd["birth"])
                else:
                    raise ValueError("unknown custody operation")
            if shared in readable:
                self.handoff(shared)
            if self.window and now() >= self.next_sweep:
                started = now()
                signal.setitimer(signal.ITIMER_REAL, 0.05)
                try:
                    self.observe()
                finally:
                    signal.setitimer(signal.ITIMER_REAL, 0)
                require(now() - started <= 50_000_000, "scout sweep exceeded its budget")
                self.message("sweep", index=self.sweeps, startNs=str(started), endNs=str(now()))
            if self.pending and self.token:
                peer, req, hpid = self.pending
                with peer:
                    self.answer(peer, req, hpid)
                self.pending = None
        require(not self.pending and self.window is None, "unclosed custody window/transfer")
        if self.known:
            close_pair(self.known)
        for handle in self.anchors:
            close_pair(handle)
        usage = resource.getrusage(resource.RUSAGE_SELF)
        self.message(
            "stopped",
            token=self.token,
            closedPairs=len(self.anchors) + int(bool(self.known)),
            windows=self.windows,
            helperTransfers=self.counts["gets"],
            maxRSSKiB=usage.ru_maxrss,
            userSeconds=usage.ru_utime,
            systemSeconds=usage.ru_stime,
            procBytes=proc_bytes,
            procReads=proc_reads,
        )


def birth_of_self():
    fd = os.open("/proc/self", os.O_DIRECTORY | os.O_RDONLY)
    try:
        return birth(fd)
    finally:
        os.close(fd)


def request_pair(config, worker, pair=None):
    # Put the request on the private reply socket before announcing it. The
    # holder never waits on a peer while its discovery sweep is scheduled.
    local, remote = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    nonce = secrets.token_hex(12)
    try:
        local.settimeout(0.7)
        data = dict(
            session=config["session"],
            cycle=config["cycle"],
            nonce=nonce,
            birth=worker,
            op="offer" if pair else "get",
            token=config["tokens"].get(str(worker["pid"])),
        )
        send(local, data, pair or ())
        with socket.socket(fileno=os.dup(3)) as shared:
            require(
                shared.sendmsg(
                    [b"H"],
                    [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [remote.fileno()]))],
                )
                == 1,
                "helper request refused",
            )
        ack, fds, _ = receive(local, [2])
        try:
            require(
                ack["session"] == config["session"]
                and ack["cycle"] == config["cycle"]
                and ack["holderPid"] == config["holderPid"]
                and ack["helperPid"] == os.getpid()
                and ack["nonce"] == nonce
                and key(ack["birth"]) == key(worker)
                and ack["descriptors"] == list(map(identity, fds))
                and os.readlink(f"/proc/self/fd/{fds[0]}") == "anon_inode:[pidfd]"
                and stat.S_ISDIR(os.fstat(fds[1]).st_mode),
                "unverified returned descriptor pair",
            )
            if data["token"]:
                require(data["token"] == ack["token"], "wrong prior token")
            return ack, fds
        except BaseException:
            for fd in fds:
                os.close(fd)
            raise
    finally:
        local.close()
        remote.close()


if __name__ == "__main__":
    parent = int(sys.argv[1])
    require(
        ctypes.CDLL(None, use_errno=True).prctl(1, 9, 0, 0, 0) == 0 and os.getppid() == parent,
        "controller lost",
    )
    resource.setrlimit(resource.RLIMIT_NOFILE, (64, 64))
    resource.setrlimit(resource.RLIMIT_AS, (96 << 20, 96 << 20))
    raw = sys.stdin.buffer.readline(MAX_PACKET + 1)
    require(0 < len(raw) <= MAX_PACKET, "contract input cap")
    first = json.loads(raw)

    def timeout_handler(signum, frame):
        raise TimeoutError("scout sweep deadline")

    signal.signal(signal.SIGALRM, timeout_handler)
    with socket.socket(fileno=3) as shared:
        shared.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        scout = Scout(first["contract"], first["session"], parent)
        scout.loop(shared)
