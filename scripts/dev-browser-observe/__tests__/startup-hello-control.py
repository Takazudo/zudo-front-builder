"""Exercise the normal sample decisions with deterministic proc and clock inputs."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("sample", Path(__file__).resolve().parents[1] / "lib/linux-rss-sample.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
root = dict(pid=10, ppid=9, pgid=10, startTicks="20", bootId="fixed", birthVersion="linux-proc-v1", state="S", comm="node")
dev = dict(root, pid=11, ppid=10, startTicks="21")
worker = dict(root, pid=12, ppid=11, pgid=12, startTicks="22")
end = dict(pidfdFlags=0, pollMask=17, observedNs="252")
for case in ["live", "terminal", "missing-status", "missing-rollup", "bad-parent", "unbound-live", "wrong-image", "closed-pair"]:
    rows = []
    s = module.Sample(dict(root=root,known=[root],terminalProofs=[],executable={},fixedDevArgs=["dev"]),rows.append)
    s.fixed_native = None if case == "bad-parent" else dev
    s.addition = dict(birth=worker,dev=dev,root=root,token="token",receipt={"verifiedNs":"250"})
    s.custody = SimpleNamespace(startup_pair=[3,4])
    s.verify_chain = lambda row,bindings: None
    s.boot = lambda: "fixed"
    s.stat = lambda pid,directory,boot: worker
    def read(path,cap,directory=None):
        if path.endswith("/fdinfo/3"): return b"Pid:\t12\n"
        if path == "status": return b"" if case=="missing-status" else b"VmRSS: 123 kB\n"
        assert path=="smaps_rollup"
        return b"" if case=="missing-rollup" else b"Rss: 125 kB\nPrivate_Clean: 4 kB\nPrivate_Dirty: 5 kB\nSwap: 0 kB\n"
    s.read = read
    def image(directory,row):
        if case == "wrong-image": raise module.Invalid("unexpected-executable")
        return {"workerArg":"__zfb-compiler-worker"}
    s.exe_identity = image
    s.threads = lambda directory,boot:dict(listed=1,rendererHosts=[])
    def poll(fd):
        if case=="closed-pair":raise OSError("closed descriptor")
        return end if case=="terminal" else None
    with patch.object(module,"terminal",side_effect=poll):
        try:s.observe_startup_addition({11:dev},{} if case in ("terminal","unbound-live") else {12:worker})
        except (module.Invalid,OSError):
            assert case not in ("live","terminal"),case
            if case=="missing-rollup": assert any(r.get("rssKiB")==123 for r in rows)
            continue
    assert case in ("live","terminal"),case
    obs,=[r["process"] for r in rows if r["type"]=="observation"]
    assert obs["fromStartupHello"] is True
    if case=="live":
        assert obs["disposition"]=="live" and obs["rssKiB"]==125 and obs["statusKiB"]["VmRSS"]==123 and obs["smapsRollupKiB"]["Rss"]==125
    else:
        assert obs["disposition"]=="custodied-departure" and obs["terminalProof"]["pollMask"]==17
        assert obs["rssKiB"] is None and not [r for r in rows if r["type"]=="rss"]
# The offered pair occupies a slot before the scan binds any candidate.
# With just enough room for root and the addition, refuse the native child
# before opening another pidfd. Failure must still close the original copies.
rows = []
closes = []
s = module.Sample(dict(root=root, known=[root], terminalProofs=[], executable={},
                       fixedDevArgs=["dev"]), rows.append)
s.addition = dict(birth=worker)
s.custody = SimpleNamespace(close_startup_pair=lambda: closes.append(True))
bound = []
def bind(stack, candidate):
    bound.append(candidate["pid"])
    return candidate, 3, 4
s.bind = bind
s.check_root = lambda binding: None
s.boot = lambda: "fixed"
s.rows = lambda directory, cap: [dev["pid"]]
s.read = lambda path, cap: b"stat"
with patch.object(module, "MAX_OWNED", 2), patch.object(module, "stat_row", return_value=dev):
    try:
        s.run()
    except module.Invalid as exc:
        assert str(exc) == "owned-process-cap", exc
    else:
        raise AssertionError("added birth escaped incremental ownership cap")
assert bound == [root["pid"]] and closes == [True]
print("PASS")
