"""SCM_RIGHTS custody of already verified fixed compiler births, session-local."""
import array
import contextlib
import ctypes
import json
import hashlib
import os
import re
import resource
import secrets
import select
import socket
import stat
import struct
import sys
import time
import threading
import importlib.util

MAX_OWNED = 64
MAX_PACKET = 2048
MAX_FDS = 2
MAX_PAIRS = 1
U32 = struct.Struct("3i")
CLOEXEC = socket.MSG_CMSG_CLOEXEC


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
        require(payload and not flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC) and valid and
                len(fds) in want and len(creds) == int(credential), "invalid custody response")
        return json.loads(payload), fds, creds[0] if creds else None
    except BaseException:
        for fd in fds:
            os.close(fd)
        raise


def birth_key(birth):
    return tuple(birth[key] for key in ("bootId", "pid", "ppid", "pgid", "startTicks"))


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


def confirm_birth(raw, birth):
    head, sep, tail = raw.rpartition(b")")
    pid, delim, _ = head.partition(b"(")
    fields = tail.split()
    require(sep and delim and pid.strip() == str(birth["pid"]).encode() and len(fields) >= 20 and
            fields[1] == str(birth["ppid"]).encode() and fields[2] == str(birth["pgid"]).encode() and
            fields[19] == birth["startTicks"].encode(), "custody birth mismatch")


def verify_pair(fds, msg, expected_root):
    fd, directory = fds
    birth, parent = msg["birth"], msg["parent"]
    require(expected_root and parent["pid"] == birth["ppid"] and
            parent["ppid"] == expected_root["pid"] and parent["pgid"] == expected_root["pgid"] and
            birth["pgid"] == birth["pid"], "custody worker ancestry")
    require(read_small("/proc/sys/kernel/random/boot_id").strip().decode() ==
            birth["bootId"] == parent["bootId"] == expected_root["bootId"], "custody boot mismatch")
    require(msg["descriptors"] == list(map(identity, fds)) and
            os.readlink(f"/proc/self/fd/{fd}") == "anon_inode:[pidfd]" and
            stat.S_ISDIR(os.fstat(directory).st_mode), "invalid custody descriptor pair")
    proc = os.stat(f'/proc/{birth["pid"]}')
    require(identity(directory) == [str(proc.st_dev), str(proc.st_ino)], "custody wrong process directory")
    confirm_birth(read_small("stat", directory), birth)
    for ancestor in (parent, expected_root):
        confirm_birth(read_small(f'/proc/{ancestor["pid"]}/stat'), ancestor)
    named = re.findall(rb"(?m)^Pid:\s*(-?\d+)$", read_small(f"/proc/self/fdinfo/{fd}"))
    require(named == [str(birth["pid"]).encode()], "custody unpaired pidfd")


class Client:
    def __init__(self, config):
        self.session, self.epoch = config["session"], config["epoch"]
        self.tokens = config["tokens"]
        self.holder_pid = config["holderPid"]
        self.last_receipt = None
        self.startup_addition = None
        self.startup_pair = []
        self.peer, transfer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        try:
            with socket.socket(fileno=3) as shared, transfer:
                # Exactly one byte travels over Node's inherited stream. All
                # bidirectional packet traffic uses this helper's unique socket.
                n = shared.sendmsg([b"H"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS,
                                            array.array("i", [transfer.fileno()]))])
                require(n == 1, "custody rendezvous failed")
            self.peer.settimeout(0.7)
            hello = dict(op="hello", session=self.session, epoch=self.epoch, root=config["root"])
            optional = config.get("allowStartupAddition") is True
            if optional:
                hello["allowStartupAddition"] = True
            response, received = self.exchange(hello, expect=(0, 2) if optional else (0,))
            try:
                addition = response.get("startupAddition")
                require(bool(received) == (addition is not None), "unpaired startup hello")
                if addition is not None:
                    require(optional and not self.tokens and len(received) == 2, "unexpected startup addition")
                    worker, dev, root = addition["birth"], addition["dev"], addition["root"]
                    require(birth_key(root) == birth_key(config["root"]) and
                            root["pid"] == dev["ppid"] and root["pgid"] == dev["pgid"] and
                            worker["ppid"] == dev["pid"] and worker["pid"] == worker["pgid"] and
                            worker["bootId"] == dev["bootId"] == root["bootId"] and
                            re.fullmatch("[0-9a-f]{32}", addition["token"]) and
                            int(addition["acquiredNs"]) <= int(response["verifiedNs"]),
                            "invalid added worker lineage")
                    require(addition["descriptors"] == list(map(identity, received)) and
                            os.readlink(f"/proc/self/fd/{received[0]}") == "anon_inode:[pidfd]" and
                            stat.S_ISDIR(os.fstat(received[1]).st_mode), "wrong startup hello pair")
                    self.startup_addition = dict(addition, receipt=dict(self.last_receipt,
                                                    descriptors=addition["descriptors"]))
                    self.startup_pair = received
                    received = []
            finally:
                for fd in received:
                    os.close(fd)
        except BaseException:
            self.peer.close()
            raise

    def exchange(self, data, fds=(), expect=0):
        send(self.peer, data, fds)
        msg, received, _ = receive(self.peer, expect if isinstance(expect, tuple) else [expect])
        try:
            require(msg.get("ok") is True and msg["epoch"] == self.epoch and
                    msg["holderPid"] == self.holder_pid and msg["helperPid"] == os.getpid() and
                    msg["session"] == self.session, "unverified custody ACK")
            self.last_receipt = {k: msg[k] for k in ("session","epoch","helperPid","holderPid","verifiedNs")}
            if "descriptors" in msg:
                self.last_receipt["descriptors"] = msg["descriptors"]
            return msg, received
        except BaseException:
            for fd in received:
                os.close(fd)
            raise

    def retain(self, birth, binding, parent):
        token = self.tokens.get(str(birth["pid"]))
        if token:
            # A previous token alone does not prove that this holder still owns
            # both original descriptors for the current verified live birth.
            dup = self.retrieve(birth)
            try:
                require(identity(dup[1]) == identity(binding[2]), "changed custody process directory")
                paired = re.findall(rb"(?m)^Pid:\s*(-?\d+)$", read_small(f"/proc/self/fdinfo/{dup[0]}"))
                require(paired == [str(birth["pid"]).encode()], "live custody pidfd contradiction")
            finally:
                for fd in dup:
                    os.close(fd)
            return token
        _, fd, directory = binding
        msg, _ = self.exchange(dict(op="retain", birth=birth, parent=parent, descriptors=[identity(fd),identity(directory)]),
                               [fd, directory])
        token = msg["token"]
        require(re.fullmatch("[0-9a-f]{32}", token), "invalid custody receipt")
        return token

    def retrieve(self, birth):
        token = self.tokens[str(birth["pid"])]
        msg, fds = self.exchange(dict(op="get", birth=birth, token=token), expect=2)
        try:
            require(msg.get("descriptors") == list(map(identity, fds)) and
                    os.readlink(f"/proc/self/fd/{fds[0]}") == "anon_inode:[pidfd]" and
                    stat.S_ISDIR(os.fstat(fds[1]).st_mode), "custody retrieved descriptor mismatch")
            return fds
        except BaseException:
            for fd in fds:
                os.close(fd)
            raise

    def release(self, birth, token):
        self.exchange(dict(op="release", birth=birth, token=token))

    def close_startup_pair(self):
        # Detach the ownership list before closure. Even when one close
        # reports a failure, ExitStack closes its sibling without retrying a
        # potentially reused descriptor number.
        pair, self.startup_pair = self.startup_pair, []
        with contextlib.ExitStack() as cleanup:
            for fd in pair:
                cleanup.callback(os.close, fd)

    def close(self):
        try:
            self.close_startup_pair()
        finally:
            self.peer.close()


def retainable_startup_state(pidfd, worker):
    """A live startup worker must still be live. A completed transient compiler
    worker, verified by the scout as a status-0 zombie, must still be terminal."""
    poll = select.poll()
    poll.register(pidfd, select.POLLIN)
    ended = bool(poll.poll(0))
    transient = worker.get("completedTransient")
    if transient is None:
        return not ended
    return (ended and worker.get("state") == "Z" and transient.get("exitCode") == 0 and
            transient.get("role") == "__zfb-compiler-worker")


def startup_retention_birth(scout, pair, worker, dev):
    """The birth the holder retains: settled, then re-polled under the ownership lock."""
    worker = settle_startup_worker(scout, pair, worker, dev)
    require(retainable_startup_state(pair[0], worker), "startup worker exited before retention")
    return worker


def settle_startup_worker(scout, pair, worker, dev):
    """A live-verified worker that ended before retention is re-read by the scout:
    a clean zombie becomes a completed transient role, a reaped one raises the
    scout's WorkerGone and the next sweep decides."""
    poll = select.poll()
    poll.register(pair[0], select.POLLIN)
    if worker.get("completedTransient") is None and poll.poll(0):
        return scout.settle_published(pair, worker, dev)
    return worker


def admit_startup_completion(first_retained, held):
    require(first_retained is None and not held, "startup completion after retention")


def duplicate_startup_pair(pair):
    first = os.dup(pair[0])
    try:
        return [first, os.dup(pair[1])]
    except BaseException:
        os.close(first)
        raise


def serve(shared, parent, session, startup_mode=False):
    # Parent is the only process that creates and distributes the shared endpoint.
    # SO_PEERCRED on that inherited socket describes its original creator, so
    # require kernel credentials on each helper's one-byte rendezvous instead.
    shared.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
    held = {}
    high_water = 0
    retains = gets = releases = 0
    epoch = 0
    root = None
    scout = None
    ownership_lock = threading.Lock()
    startup_key = None
    first_retained = None
    startup_binding = None
    hello_transfers = 0

    def complete(record):
        # The scout bound zfb live but never saw its config worker. Nothing is
        # retained; the controller confirms the record against readiness later.
        nonlocal startup_key, first_retained
        with ownership_lock:
            admit_startup_completion(first_retained, held)
            first_retained = ("product-completion",)
            print(json.dumps(dict(startupCompletion=record, session=session,
                                  publishedNs=str(time.monotonic_ns()))), flush=True)

    def publish(pair, worker, dev, launch):
        nonlocal retains, high_water, startup_key, first_retained, startup_binding
        if pair is None:
            print(json.dumps(dict(startupReport=scout.report(), session=session)), flush=True)
            return
        key = birth_key(worker)
        descriptors = list(map(identity, pair))
        with ownership_lock:
            require(first_retained is None or key == first_retained, "second startup compiler birth")
            require(not held or key in held, "ambiguous startup retention")
            # A concurrent ordinary epoch may have held the lock since the
            # scout's last live check. Take a new nonblocking group poll here.
            worker = startup_retention_birth(scout, pair, worker, dev)
            live_ns = time.monotonic_ns()
            require(live_ns < scout.end_ns and not scout.stop.is_set(), "startup handoff expired")
            first_retained = key
            if key in held:
                token, original = held[key]
                require(list(map(identity, original)) == descriptors, "conflicting startup pair")
            else:
                token = secrets.token_hex(16)
                original = duplicate_startup_pair(pair)
                held[key] = (token, original)
                retains += 1
                high_water = 1
            startup_key = key
            acquired = str(time.monotonic_ns())
            startup_binding = dict(birth=worker, dev=dev, root=launch, token=token,
                                   descriptors=descriptors, acquiredNs=acquired)
            print(json.dumps(dict(startupBirth=worker, dev=dev, root=launch, token=token,
                                  descriptors=descriptors, session=session,
                                  liveVerifiedNs=str(live_ns), acquiredNs=acquired)), flush=True)

    print(json.dumps(dict(ready=True, pid=os.getpid(), session=session,
                          timeNamespace=os.readlink("/proc/self/ns/time"))), flush=True)
    while True:
        readable, _, _ = select.select([shared, sys.stdin], [], [])
        if sys.stdin in readable:
            if not startup_mode:
                require(sys.stdin.buffer.read(1) == b"", "invalid custody stop")
                break
            line = sys.stdin.buffer.readline(16385)
            if not line:
                break
            require(startup_mode and scout is None and len(line) <= 16384, "unexpected custody seed")
            seed = json.loads(line)
            require(seed["session"] == session and seed["root"]["ppid"] == parent
                    and seed["root"]["pid"] == seed["root"]["pgid"], "invalid scout root")
            spec = importlib.util.spec_from_file_location("fixed_startup",
                    os.path.join(os.path.dirname(__file__), "linux-fixed-startup.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            scout = module.Startup(seed["fixed"], parent, seed["root"], publish, complete)
            scout.thread.start()
            continue
        ancillary_cap = socket.CMSG_SPACE(MAX_FDS * 4) + socket.CMSG_SPACE(U32.size)
        raw, ancillary, flags, _ = shared.recvmsg(1, ancillary_cap, CLOEXEC)
        # Any malformed transfer exits the holder; the kernel closes every FD.
        creds = [U32.unpack(v) for l, t, v in ancillary if (l,t) == (socket.SOL_SOCKET,socket.SCM_CREDENTIALS)]
        rights = [array.array("i", v) for l, t, v in ancillary if (l,t) == (socket.SOL_SOCKET,socket.SCM_RIGHTS)]
        require(raw == b"H" and not flags & (socket.MSG_CTRUNC|socket.MSG_TRUNC) and len(ancillary)==2 and
                len(creds)==1 and creds[0][1]==os.getuid() and len(rights)==1 and len(rights[0])==1,
                "invalid custody rendezvous")
        peer_pid = creds[0][0]
        with open(f"/proc/{peer_pid}/stat", "rb") as proc:
            fields = proc.read(4097).rsplit(b")", 1)[1].split()
        require(int(fields[1]) == parent, "foreign custody peer")
        with socket.socket(fileno=rights[0][0]) as peer:
            require(peer.type == socket.SOCK_SEQPACKET, "non-packet custody socket")
            peer.settimeout(0.75)
            hello, _, _ = receive(peer, [0])
            epoch += 1
            candidate = hello.pop("root")
            offer = "allowStartupAddition" in hello
            if offer:
                require(startup_mode and hello.pop("allowStartupAddition") is True, "unsupported startup hello")
            require(hello == dict(op="hello",session=session,epoch=epoch), "custody session/epoch mismatch")
            require(candidate["pid"] == candidate["pgid"] and candidate["ppid"] == parent,
                    "custody foreign root")
            if root is None:
                confirm_birth(read_small(f'/proc/{candidate["pid"]}/stat'), candidate)
                root = candidate
            require(birth_key(candidate) == birth_key(root), "custody root changed")
            def ack(extra=None, fds=(), stamp=None):
                result=dict(ok=True,epoch=epoch,helperPid=peer_pid,holderPid=os.getpid(),
                            session=session,verifiedNs=stamp or str(time.monotonic_ns()))
                result.update(extra or {})
                send(peer,result,fds)
            # One nonwaiting snapshot. An acquisition after this handshake is
            # not part of this request. The controller still reconciles the full
            # population at closure, including later discoveries.
            sent_addition = False
            if offer and ownership_lock.acquire(blocking=False):
                try:
                    if startup_binding and startup_key in held and hello_transfers == 0:
                        token, original = held[startup_key]
                        require(token == startup_binding["token"], "changed startup ownership")
                        stamp = str(time.monotonic_ns())
                        print(json.dumps(dict(startupHello=dict(epoch=epoch, helperPid=peer_pid,
                              holderPid=os.getpid(), verifiedNs=stamp, token=token), session=session)), flush=True)
                        ack(dict(startupAddition=startup_binding), original, stamp)
                        hello_transfers += 1
                        sent_addition = True
                finally:
                    ownership_lock.release()
            if not sent_addition:
                ack()
            for _ in range(2*MAX_OWNED+1):
                # Peek EOF without consuming any rights from a new request.
                if not peer.recv(1, socket.MSG_PEEK):
                    break
                msg, fds, _ = receive(peer, [0,2])
                key = birth_key(msg["birth"])
                action = msg["op"]
                with ownership_lock:
                    if action == "retain":
                        if startup_mode:
                            require(first_retained is None or first_retained == key, "second compiler birth")
                            first_retained = key
                        require(len(fds) == 2 and (not held or (startup_mode and key == startup_key and key in held)), "duplicate or excessive custody")
                        verify_pair(fds, msg, root)
                        if key in held:
                            token, original = held[key]
                            require(list(map(identity, fds)) == list(map(identity, original)), "changed startup pair")
                            for fd in fds:
                                os.close(fd)
                            fds = original
                        else:
                            token = secrets.token_hex(16)
                            held[key] = (token, fds)
                            retains += 1
                        high_water=max(high_water,len(held))
                        ack(dict(token=token,descriptors=list(map(identity,fds))))
                    else:
                        token, existing = held[key]
                        require(not fds and secrets.compare_digest(token, msg["token"]), "unverified custody token")
                        if action == "get":
                            gets += 1
                            ack(dict(descriptors=list(map(identity,existing))),existing)
                        else:
                            require(action == "release", "unsupported custody operation")
                            released = list(map(identity, existing)) if startup_mode else None
                            for fd in existing:
                                os.close(fd)
                            del held[key]
                            releases += 1
                            ack(dict(descriptors=released) if startup_mode else None)
            else:
                raise ValueError("custody operation cap")
    if scout is not None:
        scout.stop.set()
        scout.thread.join(0.15)
        require(not scout.thread.is_alive(), "startup scout still running")
    lines = sorted("|".join(map(str, (*key, record[0]))) for key,record in held.items())
    digest = hashlib.sha256("\n".join(lines).encode()).hexdigest()
    for _, fds in held.values():
        for fd in fds:
            os.close(fd)
    usage=resource.getrusage(resource.RUSAGE_SELF)
    print(json.dumps(dict(stopped=True,pairsClosed=len(held),retainedDigest=digest,epochs=epoch,
                          maxPairs=high_water,retains=retains,gets=gets,releases=releases,maxRssKiB=usage.ru_maxrss,
                          **({"startupHelloTransfers":hello_transfers} if startup_mode else {}),
                          userSeconds=usage.ru_utime,systemSeconds=usage.ru_stime)),flush=True)


if __name__ == "__main__":
    parent = int(sys.argv[1])
    require(ctypes.CDLL(None).prctl(1,9,0,0,0)==0 and os.getppid()==parent, "custody controller gone")
    resource.setrlimit(resource.RLIMIT_NOFILE,(160,160))
    resource.setrlimit(resource.RLIMIT_AS,(96<<20,96<<20))
    session=sys.stdin.buffer.readline(128).decode().strip()
    require(re.fullmatch("[0-9a-f]{32}",session),"invalid custody session")
    with socket.socket(fileno=3) as shared:
        serve(shared,parent,session,sys.argv[2:] == ["--startup"])
