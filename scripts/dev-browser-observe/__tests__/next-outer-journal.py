"""Exercise the actual wrapper's failure finalizer without creating any OS child."""
import io
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile
from unittest.mock import patch

lib=Path(__file__).resolve().parents[1]/"lib"
wrapper=lib/"next-outer-run.py"
life=runpy.run_path(str(lib/"next-outer-lifecycle.py"))

class Owned:
    pid=42
    def __init__(self):
        self.signals=[]
        self.waits=[]
    def poll(self):
        return None
    def send_signal(self,sig):
        self.signals.append(sig)
    def wait(self,timeout):
        self.waits.append(timeout)
        raise subprocess.TimeoutExpired("virtual child only",timeout)

with tempfile.TemporaryDirectory() as stage:
    root=Path(stage)
    state=root/".local/state/zfb/dev-browser-observe"
    cwd=Path.cwd()
    os.chdir(root)
    try:
        install=root/"install"
        install.mkdir()
        (install/"install-receipt.json").write_text(json.dumps(dict(node=dict(realpath=sys.executable))))
        profile=root/"profile.json"
        profile.write_text(json.dumps(dict(fixedRssSafety="linux-fixed-peer.v4",framework="next",
            outDir=str(root/"output"),limits=dict(wallCeilingMs=7200000),distribution=dict(installDir=str(install)),
            fixedNextOuterObserver=dict(provider="linux-next-outer-receipt.v1",cwd=str(root),
              scriptPath=str(wrapper),pythonPath=str(Path(sys.executable).resolve())))))
        owned=Owned()
        real_sync=os.fsync
        launched=False
        def spawn(*a,**kw):
            global launched
            # Claim and original identity were durable before the fake Popen.
            initial=json.loads((state/"next-outer-active.json").read_text())
            assert initial["stage"]=="pre-launch" and initial["outer"]["pid"]==os.getpid()
            launched=True
            return owned
        def sync(fd):
            if launched:raise OSError("persistent evidence EIO")
            real_sync(fd)
        stderr=io.StringIO()
        with patch.object(Path,"home",return_value=root),patch.object(subprocess,"Popen",side_effect=spawn), \
             patch.object(os,"pidfd_open",side_effect=OSError("original pidfd missing")), \
             patch.object(os,"fsync",side_effect=sync),patch.object(sys,"stderr",stderr), \
             patch.object(sys,"argv",[str(wrapper),str(profile),"controlled-outer"]):
            try:runpy.run_path(str(wrapper),run_name="__main__")
            except SystemExit as end:assert end.code==1
            else:raise AssertionError("incomplete outer wrapper accepted")
        assert owned.signals==[signal.SIGTERM,signal.SIGKILL]
        assert len(owned.waits)==2 and 0<owned.waits[0]<=55 and 0<owned.waits[1]<=60
        assert (state/"next-outer-active.json").exists()
        assert (state/"active.json").exists() and (root/"controlled-outer-outer-hold.json").exists()
        assert 'original pidfd missing' in (root/"controlled-outer-outer.jsonl").read_text()
        assert 'write-failed' in stderr.getvalue()
    finally:os.chdir(cwd)

with tempfile.TemporaryDirectory() as stage:
    root=Path(stage)
    first=root/"missing"/"bad"
    # Ineligible local destination must not suppress a usable second destination.
    (root/"missing").write_text("foreign")
    good=root/"positive.json"
    statuses=life["preserve_hold"]([first,good],{"unresolved":["test"]})
    assert [s["state"] for s in statuses]==["write-failed","created"]
    marker=life["OwnershipMarker"](root/"owner",dict(phase="owned"))
    (root/"owner").rename(root/"moved");(root/"owner").write_text("foreign")
    try:marker.release()
    except ValueError:pass
    else:raise AssertionError("foreign marker removed")
    finally:marker.close()
    assert (root/"owner").read_text()=="foreign"
disposition=life["disposition"]
for args,want in [((0,True,True,"body"),"verified-body"),
                  ((1,True,True,"body-classified"),"verified-body-classified"),
                  ((1,True,True,"decision-only"),"verified-no-write"),
                  ((3,True,False,"prewrite-refused"),"normal-refusal")]:
    assert disposition(*args)==want,(args,want)
for args in [(1,True,True,"body"),(0,True,True,"body-classified"),(8,True,True,"body-classified"),
             (1,True,False,"body-classified"),(1,False,True,"body-classified"),
             (0,True,True,"incomplete"),(1,True,True,"body-stale")]:
    try:disposition(*args)
    except ValueError:pass
    else:raise AssertionError(args)
print("persistent-EIO cleanup, prior durable ownership, independent holds, original-inode preservation and terminal dispositions passed")
