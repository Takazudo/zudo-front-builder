"""Opt-in transport in the independent WSL Python wrapper that already inventories the browser."""

import importlib.util
import json
import os
import re
import resource
import secrets
import selectors
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path


def need(value, message):
    if not value:
        raise ValueError(message)


def file_json(path, cap):
    with path.open("rb") as stream:
        data=stream.read(cap+1)
    need(0<len(data)<=cap,"JSON input cap")
    return json.loads(data)


def proc_birth(pid):
    raw=Path(f"/proc/{pid}/stat").read_bytes()
    need(len(raw)<=4096,"proc stat cap")
    row=raw.rsplit(b") ",1)[1].split()
    return dict(pid=pid,ppid=int(row[1]),pgid=int(row[2]),startTicks=row[19].decode(),
                birthVersion="linux-proc-v1",bootId=boot)


def image(path):
    st=path.stat()
    return dict(path=str(path),device=str(st.st_dev),inode=str(st.st_ino))


def browser_inventory():
    # The previous wrapper samples only once after normal browser setup. Keep
    # its 250ms schedule separate from pidfd/socket readiness and never fetch a route.
    marker=Path.home()/".local/state/zfb/dev-browser-observe/active.json"
    try:
        if not (file_json(directory/"receipt.json",2<<20).get("browser") or {}).get("executableSha256"):
            return False
        entry=file_json(marker,262144)
    except FileNotFoundError:
        return False
    if entry.get("labelDir")!=str(directory):
        return False
    members={cli.pid:proc_birth(cli.pid)}
    pending=[cli.pid]
    while pending:
        pid=pending.pop()
        try:
            task=Path(f"/proc/{pid}/task")
            workers=list(task.iterdir())
            need(len(workers)<=256,"browser inventory task cap")
            for t in workers:
                row=(t/"children").read_bytes()
                need(len(row)<=4096,"browser inventory children input cap")
                for value in row.split():
                    child=int(value)
                    if child not in members:
                        members[child]=proc_birth(child)
                        pending.append(child)
                        need(len(members)<=128,"browser inventory population cap")
        except (FileNotFoundError,ProcessLookupError):
            continue
    if entry.get("controllerPid") not in members:
        return False
    result=[]
    for pid,original in members.items():
        try:
            exe=Path(f"/proc/{pid}/exe")
            st=exe.stat();path=os.readlink(exe)
            if original==proc_birth(pid):
                result.append({**original,"path":path,"device":st.st_dev,"inode":st.st_ino})
        except (FileNotFoundError,ProcessLookupError):
            continue
    browsers=[r for r in result if Path(r["path"]).name in ("chrome","chrome-headless-shell","headless_shell")]
    if not browsers:return False
    with browser_file.open("x") as stream:
        json.dump(dict(bootId=boot,controller=entry["controllerPid"],cli=cli.pid,
                       collectedNs=now(),members=result,browser=browsers),stream)
        stream.flush();os.fsync(stream.fileno())
    return True


if __name__=="__main__":
    now=time.monotonic_ns
    started=now()
    profile_path=Path(sys.argv[1]);label=sys.argv[2]
    need(len(sys.argv)==3 and profile_path.is_absolute() and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}",label),
         "outer arguments")
    profile=file_json(profile_path,1<<20)
    spec=profile["fixedNextOuterObserver"]
    root=Path.cwd()
    need(profile["fixedRssSafety"]=="linux-fixed-peer.v4" and profile["framework"]=="next" and
         spec["provider"]=="linux-next-outer-receipt.v1" and spec["cwd"]==str(root) and
         Path(__file__).resolve()==Path(spec["scriptPath"]) and
         Path(sys.executable).resolve()==Path(spec["pythonPath"]), "outer provider admission")
    install=file_json(Path(profile["distribution"]["installDir"])/"install-receipt.json",1<<20)
    node=Path(install["node"]["realpath"]);lib=Path(__file__).resolve().parent
    argv=[str(node),str(lib.parent/"cli.mjs"),"run","--profile",str(profile_path),"--label",label]
    paths=dict(node=image(node),python=image(Path(sys.executable).resolve()),cwd=str(root),
               supervisorArgv=argv,workerScript=str(lib/"session-worker.mjs"),
               holderScript=str(lib/"next-peer-custody.py"),label=label,profilePath=str(profile_path))
    sub=importlib.util.spec_from_file_location("outer_observer",lib/"next-outer-observer.py")
    reader=importlib.util.module_from_spec(sub);sub.loader.exec_module(reader)
    boot=Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    self_birth=proc_birth(os.getpid())
    need(profile["limits"]["wallCeilingMs"]==7200000,"outer wall contract")
    end=started+7200_000_000_000
    nonce=secrets.token_hex(16)
    directory=Path(profile["outDir"])/label
    browser_file=root/(label+"-owned-browser.json")
    logfile=root/(label+"-outer.jsonl")
    written=lines=0
    log=logfile.open("x",buffering=1)
    def record(row):
        global lines,written
        line=json.dumps(dict(outerNs=str(now()),**row),ensure_ascii=True,separators=(",",":"))+"\n"
        written+=len(line);lines+=1
        need(len(line)<=50*1024 and written<=192*1024 and lines<=16,"outer journal cap")
        log.write(line);log.flush();os.fsync(log.fileno())
    pairs=[socket.socketpair(socket.AF_UNIX,k) for k in (socket.SOCK_SEQPACKET,socket.SOCK_SEQPACKET,socket.SOCK_STREAM)]
    for owner,child in pairs:owner.setblocking(False)
    pairs[0][0].setsockopt(socket.SOL_SOCKET,socket.SO_PASSCRED,1)
    for endpoint in pairs[1]:endpoint.setsockopt(socket.SOL_SOCKET,socket.SO_PASSCRED,1)
    fds=[child.fileno() for owner,child in pairs]
    declaration=dict(provider=spec["provider"],nonce=nonce,outer=self_birth,fds=fds,
                     sockets=[os.readlink(f"/proc/self/fd/{fd}") for fd in fds])
    env={**os.environ,"ZFB_NEXT_OUTER_CHANNEL":json.dumps(declaration,separators=(",",":"))}
    sel=selectors.DefaultSelector()
    cli=obs=pidfd=None
    result=None;failure=None;captured=False
    try:
        cli=subprocess.Popen(argv,pass_fds=fds,env=env)
        pidfd=os.pidfd_open(cli.pid)
        supervisor=proc_birth(cli.pid)
        need(supervisor["ppid"]==os.getpid(),"foreign CLI")
        for _,child in pairs:child.close()
        sel.register(pidfd,selectors.EVENT_READ,"exit")
        sel.register(pairs[1][0],selectors.EVENT_READ,"header")
        nxt=now()
        record(dict(stage="started",outer=self_birth,supervisor=supervisor,descriptors=declaration["sockets"]))
        while result is None:
            stamp=now()
            need(stamp<end,"outer supervisor wall cutoff")
            if obs and obs.state!="retired": obs.check_budget()
            if stamp>=nxt:
                if not captured:captured=browser_inventory()
                nxt=now()+250_000_000
            wake=min(nxt,end)
            if obs and obs.state!="retired":wake=min(wake,obs.deadline)
            if obs and obs.state=="observing":
                next_view=obs.views[0]+30_000_000 if obs.views else stamp
                if now()>=next_view:
                    obs.observe_live()
                    if obs.state=="qualified":sel.register(obs.pair[0],selectors.EVENT_READ,"terminal")
                    continue
                wake=min(wake,next_view)
            for key,_ in sel.select(max(0,wake-now())/1e9):
                if key.data=="exit":
                    result=cli.wait(timeout=1)
                    break
                if key.data=="header":
                    # The same private socket is bidirectional. The kernel
                    # identifies the real sender of this header independently.
                    if obs is not None:raise ValueError("duplicate or closed reverse header")
                    data=reader.receive_authority(pairs[1][0],self_birth,supervisor,nonce,paths)
                    obs=reader.OuterObservation(data["contract"],data["holder"],data["worker"],
                        nonce,data["token"],data["descriptors"],min(end,now()+3_000_000_000),record)
                    record(dict(stage="authenticated-header",controller=data["controller"],
                                holder=data["holder"],worker=data["worker"],descriptors=data["descriptors"]))
                    sel.register(pairs[0][0],selectors.EVENT_READ,"pair")
                elif key.data=="pair":
                    sel.unregister(key.fileobj);obs.accept_original(pairs[0][0])
                    record(dict(stage="authenticated-original",descriptors=obs.descriptors))
                else:
                    sel.unregister(key.fileobj)
                    packet=obs.terminal()
                    packet["sentNs"]=str(now())
                    body=json.dumps(packet,separators=(",",":")).encode()
                    need(len(body)<=4096 and pairs[1][0].send(body)==len(body),"outer terminal send")
                    need(pairs[2][0].send(b"G")==1,"outer wake send")
                    record(dict(stage="sent",packet=packet))
                    # Only one authority is accepted. Closing our write end
                    # provides normal EOF to the controller after its notification.
                    sel.unregister(pairs[1][0])
                    pairs[2][0].shutdown(socket.SHUT_WR)
        need(captured,"independent browser inventory absent")
    except BaseException as exc:
        failure=repr(exc)
        record(dict(stage="failure",error=failure))
    finally:
        if obs:obs.close()
        sel.close()
        for owner,child in pairs:owner.close();child.close()
        if cli and cli.poll() is None:
            # Direct, unreaped child identity and pidfd remain ours. The normal
            # CLI's SIGTERM handler owns its controller and reserve for teardown.
            if pidfd is not None:signal.pidfd_send_signal(pidfd,signal.SIGTERM)
            try:result=cli.wait(timeout=60)
            except subprocess.TimeoutExpired:failure=failure or "independent supervisor failed to finalize"
        if pidfd is not None:os.close(pidfd)
        usage=resource.getrusage(resource.RUSAGE_SELF)
        record(dict(stage="closed",rc=result,failure=failure,browserInventory=captured,
                    observerMaxRssKiB=usage.ru_maxrss,observerUserSeconds=usage.ru_utime,
                    observerSystemSeconds=usage.ru_stime))
        log.close()
    sys.exit(1 if failure or result is None else result if result>=0 else 128-result)
