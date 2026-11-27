"""Independent, one-use original-pair observer for the existing Python outer process."""

import fcntl
import importlib.util
import os
import socket
import stat
import time
from pathlib import Path


def load_scout_helpers():
    # Separate Python module state in the outer process, including bounded
    # proc-read accounting. The holder's recorded role views never enter here.
    spec = importlib.util.spec_from_file_location(
        "next_outer_linux_reads", Path(__file__).with_name("next-peer-custody.py")
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class OuterObservation:
    """Caller owns passive readiness, finite wall budget and raw journal."""

    def __init__(self, contract, holder, worker, nonce, token, descriptors, deadline_ns, record):
        self.io = load_scout_helpers()
        self.contract = contract
        self.holder = holder
        self.worker = worker
        self.nonce = nonce
        self.token = token
        self.descriptors = descriptors
        self.deadline = deadline_ns
        self.record = record
        self.pair = None
        self.anchors = []
        self.views = []
        self.state = "waiting"
        self.cpu_start = time.process_time_ns()
        self.io.require(contract["framework"] == "next" and
                        contract["protocol"] == "linux-fixed-peer.v2", "outer framework contract")
        root, cli, server = contract["ancestors"]
        self.io.require(root["ppid"] == holder["ppid"] and
                        root["pid"] == root["pgid"] and
                        cli["ppid"] == root["pid"] and
                        server["ppid"] == cli["pid"] and
                        worker["ppid"] == server["pid"] and
                        all(p["pgid"] == root["pid"] for p in (cli, server, worker)) and
                        all(p["bootId"] == self.io.boot for p in (root, cli, server, holder, worker)),
                        "outer ancestry")
        script = contract["script"]
        self.io.require(script["path"] == str(Path(contract["app"]) /
                        "node_modules/next/dist/compiled/jest-worker/processChild.js") and
                        script["size"] == 2868 and
                        script["sha256"] == "463a33ccdf45500486481c697ca9b7445fae5998410b82d3d5485c0b85b770eb",
                        "outer stock bootstrap pin")
        self.io.require(contract["image"]["path"] == str(Path(contract["image"]["path"]).resolve()),
                        "outer image canonical path")
        self.io.cfg = dict(image=contract["image"], app=contract["app"], server=server,
                           script=contract["script"]["path"], parentJestId=None,
                           socket=contract["socket"])

    def check_budget(self):
        self.io.require(time.monotonic_ns() < self.deadline and
                        time.process_time_ns() - self.cpu_start < 100_000_000, "outer budget")

    def close(self):
        self.state = "retired"
        if self.pair is not None:
            self.io.close_pair(self.pair)
            self.pair = None
        while self.anchors:
            self.io.close_pair(self.anchors.pop())

    def accept_original(self, sock):
        self.io.require(self.state == "waiting", "repeated original pair")
        self.state = "receiving"
        self.check_budget()
        self.io.require(sock.getblocking() is False and
                        sock.getsockopt(socket.SOL_SOCKET,socket.SO_PASSCRED) == 1,
                        "outer socket authentication unavailable")
        packet, fds, peer = self.io.receive(sock, [2], credential=True)
        self.pair = fds
        try:
            self.io.require(peer == (self.holder["pid"],os.getuid(),os.getgid()), "wrong holder")
            self.io.require(packet == {"nonce": self.nonce, "token":self.token,
                            "worker":self.worker,"descriptors":self.descriptors},
                            "wrong original pair declaration")
            holder = self.io.bind(self.holder["pid"])
            try:
                self.io.require(self.io.key(holder[2]) == self.io.key(self.holder), "holder rebirth")
            finally:
                self.io.close_pair(holder)
            pidfd, procdir = fds
            info = self.io.read_small(f"/proc/self/fdinfo/{pidfd}").splitlines()
            proc_info = os.stat(f"/proc/{self.worker['pid']}")
            self.io.require(self.io.identity(procdir) == [str(proc_info.st_dev),str(proc_info.st_ino)],
                            "original proc directory identity")
            self.io.require(b"Pid:\t" + str(self.worker["pid"]).encode() in info and
                            os.readlink(f"/proc/self/fd/{pidfd}") == "anon_inode:[pidfd]" and
                            not (fcntl.fcntl(pidfd,fcntl.F_GETFL) & ~(os.O_ACCMODE|os.O_CLOEXEC)) and
                            stat.S_ISDIR(os.fstat(procdir).st_mode) and
                            self.io.key(self.io.birth(procdir)) == self.io.key(self.worker) and
                            self.io.poll_terminal(pidfd) is None and
                            [self.io.identity(fd) for fd in fds] == self.descriptors,
                            "outer original live group pair")
            for b in self.contract["ancestors"]:
                h = self.io.bind(b["pid"])
                self.anchors.append(h)
                self.io.require(self.io.key(h[2]) == self.io.key(b),"outer ancestor birth")
            self.state = "observing"
            self.check_budget()
        except BaseException:
            self.close()
            raise

    def observe_live(self):
        try:
            self.io.require(self.state == "observing" and len(self.views) < 2,"outer live view count")
            self.check_budget()
            start = time.monotonic_ns()
            for anchor, expected in zip(self.anchors,self.contract["ancestors"]):
                view = self.io.view(anchor)
                raw = bytes.fromhex(view["argvHex"])
                wanted = b"\0".join(os.fsencode(arg) for arg in expected["argv"])+b"\0"
                good = raw == wanted
                if expected.get("retitled"):
                    good = raw.endswith(b"\0") and raw.rstrip(b"\0") == b"next-server (v16.4.6)"
                self.io.require(good and view["cwd"] == self.contract["app"] and
                                self.io.key(view["birth"])==self.io.key(expected),"outer ancestor role")
            self.io.require(view["private"] and view["dev"] and view["jestId"] is None,
                            "outer serving role")
            self.io.serving_socket(self.anchors[-1],self.io.cfg)
            self.io.require(self.io.children(self.anchors[-1][1]) == {self.worker["pid"]},
                            "ambiguous outer candidate population")
            self.io.confirm_script(self.contract["script"])
            worker_view = self.io.view((*self.pair,self.worker))
            self.io.require(self.io.is_jest(worker_view,self.io.cfg) and
                            self.io.poll_terminal(self.pair[0]) is None,"outer worker role")
            end = time.monotonic_ns()
            self.io.require(end-start < 50_000_000 and
                            (not self.views or self.views[0] < start),"overlapping/late outer view")
            self.record(dict(stage="independent-live-view", parent=view, worker=worker_view,
                             index=len(self.views)+1, startNs=str(start), endNs=str(end)))
            self.views.append(end)
            self.check_budget()
            if len(self.views)==2:
                self.state="qualified"
        except BaseException:
            self.close()
            raise

    def terminal(self):
        try:
            self.io.require(self.state=="qualified", "outer receipt unqualified")
            self.check_budget()
            mask=self.io.poll_terminal(self.pair[0])
            self.io.require(mask in (1,16,17),"group not terminal at outer readiness")
            terminal=time.monotonic_ns()
            receipt=dict(worker=self.worker, nonce=self.nonce, token=self.token,
                         descriptors=self.descriptors, pidfdFlags=0, pollMask=mask,
                         firstLiveNs=str(self.views[0]), secondLiveNs=str(self.views[1]),
                         terminalNs=str(terminal))
            self.record(dict(stage="group-terminal",proof=receipt,procReads=self.io.proc_reads,
                             procBytes=self.io.proc_bytes,cpuNs=time.process_time_ns()-self.cpu_start))
            self.check_budget()
            return receipt
        finally:
            self.close()
