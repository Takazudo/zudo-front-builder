#!/usr/bin/env python3
"""Bounded loopback evidence for two independent native runner processes.

Python serves fixtures and compares JSON; it never evaluates JavaScript. Each
case's identical source bytes go to both engines at the same bound server URL.
A two-request handshake proves parallel transport progress without timing a
latency threshold. Watchdog failures are failures, not cancellation evidence.
This harness does not prove socket closure while a host remains alive/idle.
"""
import argparse
from collections import Counter
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
import threading
import time

PREFIX = "ZFB_HOST_RECEIPT "
WATCHDOG_SECONDS = 10


class Session:
    def __init__(self):
        self.condition = threading.Condition()
        self.requests = []
        self.parallel = set()
        self.errors = []
        self.active = 0

    def snapshot(self):
        with self.condition:
            return {"accepted_requests": len(self.requests),
                    "paths": dict(Counter(row["path"] for row in self.requests)),
                    "requests": list(self.requests), "errors": list(self.errors)}


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        self.session = Session()
        super().__init__(("127.0.0.1", 0), Handler)

    def process_request(self, request, client_address):
        # Pin this connection's observer even if a failed run is followed by a
        # new session; no late handler may mutate the next run's counters.
        session = self.session
        with session.condition:
            session.active += 1

        def handle():
            try:
                Handler(request, client_address, self, session=session)
            finally:
                self.shutdown_request(request)
                with session.condition:
                    session.active -= 1
                    session.condition.notify_all()

        threading.Thread(target=handle, daemon=True).start()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def __init__(self, *args, session, **kwargs):
        self.session = session
        super().__init__(*args, **kwargs)

    def setup(self):
        super().setup()
        self.connection.settimeout(5)

    def log_message(self, *_args):
        pass

    def reply(self, status, body, headers=()):
        self.send_response_only(status)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.handle_fixture()

    def do_POST(self):
        self.handle_fixture()

    def handle_fixture(self):
        try:
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            with self.session.condition:
                self.session.requests.append({"path": self.path, "method": self.command,
                                              "body": list(body)})
            if self.path.startswith("/parallel/"):
                side = self.path.rsplit("/", 1)[1]
                with self.session.condition:
                    self.session.parallel.add(side)
                    self.session.condition.notify_all()
                    ready = self.session.condition.wait_for(
                        lambda: self.session.parallel == {"left", "right"}, timeout=5)
                if not ready:
                    raise RuntimeError("parallel request handshake did not complete")
                self.reply(200, side.encode())
            elif self.path == "/binary/data":
                self.reply(201, bytes([0, 1, 127, 128, 255]),
                           [("Content-Type", "application/octet-stream"),
                            ("Set-Cookie", "first=1"), ("Set-Cookie", "second=2"),
                            ("X-Pair", "left"), ("X-Pair", "right")])
            elif self.path in ("/redirect303/start", "/redirect307/start"):
                status = 303 if "303" in self.path else 307
                self.reply(status, b"", [("Location", self.path.replace("start", "echo"))])
            elif self.path in ("/redirect303/echo", "/redirect307/echo"):
                value = {"method": self.command, "body": list(body),
                         "contentType": self.headers.get("Content-Type"),
                         "marker": self.headers.get("X-Marker")}
                self.reply(200, json.dumps(value).encode(), [("Content-Type", "application/json")])
            else:
                # A denied/pre-aborted request reaching here is visible in the
                # independent counter even if guest code catches its outcome.
                self.reply(418, b"unexpected request reached loopback server")
        except Exception as error:
            with self.session.condition:
                self.session.errors.append(str(error))
            self.close_connection = True


def cases_for(origin):
    cases = []

    def add(name, contracts, body, expected, paths, mode="request-time"):
        cases.append({"name": name, "contracts": contracts.split(), "mode": mode,
                      "source": "(async () => { " + body + " })()",
                      "expected": expected, "expected_paths": paths})

    def url(path):
        return json.dumps(origin + path)

    add("parallel-handshake-and-microtask", "HTTP-04 BOOT-02",
        'let progressed = false; const calls = ["left","right"].map(async side => { '
        'const response = await fetch(' + url("/parallel/") + ' + side); '
        'return {side,body:await response.text(),microtaskProgressed:progressed}; }); '
        'queueMicrotask(() => { progressed = true; }); return await Promise.all(calls);',
        [{"side": side, "body": side, "microtaskProgressed": True} for side in ("left", "right")],
        {"/parallel/left": 1, "/parallel/right": 1})
    add("binary-and-duplicate-headers", "HTTP-07 WORK-04",
        'const r = await fetch(' + url("/binary/data") + '); return {status:r.status,'
        'url:r.url,redirected:r.redirected,bytes:Array.from(new Uint8Array(await r.arrayBuffer())),'
        'cookies:r.headers.getSetCookie(),pair:r.headers.get("x-pair"),'
        'headers:Array.from(r.headers)};',
        {"status": 201, "url": origin + "/binary/data", "redirected": False,
         "bytes": [0, 1, 127, 128, 255], "cookies": ["first=1", "second=2"],
         "pair": "left, right", "headers": [["content-length", "5"],
         ["content-type", "application/octet-stream"], ["set-cookie", "first=1"],
         ["set-cookie", "second=2"], ["x-pair", "left, right"]]}, {"/binary/data": 1})
    for status in (303, 307):
        root = f"/redirect{status}/"
        add(f"redirect-{status}-post", "HTTP-06 HTTP-07",
            'const r = await fetch(' + url(root + "start") + ',{method:"POST",'
            'headers:{"content-type":"application/octet-stream","x-marker":"retained"},'
            'body:new Uint8Array([0,128,255])}); return {status:r.status,url:r.url,'
            'redirected:r.redirected,echo:await r.json()};',
            {"status": 200, "url": origin + root + "echo", "redirected": True,
             "echo": {"method": "GET" if status == 303 else "POST",
                      "body": [] if status == 303 else [0, 128, 255],
                      "contentType": None if status == 303 else "application/octet-stream",
                      "marker": "retained"}}, {root + "start": 1, root + "echo": 1})
    add("pre-aborted-zero-server-requests", "CANCEL-01",
        'const controller = new AbortController(); controller.abort(new Error("pre-abort sentinel")); '
        'try { await fetch(' + url("/preabort/denied") + ',{signal:controller.signal}); '
        'return "unexpected success"; } catch(e) { return {name:e.name,message:e.message}; }',
        {"name": "Error", "message": "pre-abort sentinel"}, {})
    for name, path, raw, forge in (
        ("build-denial-zero-server-requests", "/build/denied", False, False),
        ("raw-denial-zero-server-requests", "/raw/denied", True, False),
        ("forged-raw-denial-zero-server-requests", "/forged/denied", True, True),
    ):
        call = ('globalThis.__zfbHost.fetch({url:' + url(path) + ',method:"GET",headers:[],'
                'redirect:"follow",hasBody:false,timeoutMs:null,cancelId:null},new Uint8Array(0))'
                if raw else 'fetch(' + url(path) + ')')
        prefix = 'globalThis.__zfb = {mode:"request-time",limits:{maxSubrequests:99999}}; ' if forge else ''
        message = (f"fetch() called from SSG runtime (url={origin + path}). The embedded V8 host "
                   "does not support outgoing network requests during build-time render. "
                   "Move the data fetch to a build step or a runtime-only branch.")
        add(name, "HTTP-01 HTTP-03", prefix + 'try { await ' + call + '; '
            'return "unexpected success"; } catch(e) { return {name:e.name,message:e.message}; }',
            {"name": "TypeError" if raw else "Error", "message": message}, {}, mode="build-time")
    return cases


def canonical(value):
    # Preserve JSON type distinctions (Python's True == 1 is insufficient).
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def run(binary, engine, case, server):
    deadline = time.monotonic() + WATCHDOG_SECONDS
    session = Session()
    server.session = session
    wire = json.dumps({"kind": "script", "source": case["source"], "mode": case["mode"]}) + "\n"
    failures = []
    receipt = None
    result = {"engine": engine}
    try:
        process = subprocess.run([str(binary)], input=wire, text=True, capture_output=True,
                                 timeout=WATCHDOG_SECONDS, check=False)
        result.update(exit_code=process.returncode, stdout=process.stdout, stderr=process.stderr)
        if process.returncode != 0:
            failures.append("runner exited unsuccessfully")
        lines = [line[len(PREFIX):] for line in process.stdout.splitlines() if line.startswith(PREFIX)]
        if len(lines) != 1:
            failures.append(f"expected one framed receipt, received {len(lines)}")
        else:
            try:
                receipt = json.loads(lines[0])
                if not isinstance(receipt, dict):
                    raise ValueError("receipt is not an object")
                if receipt.get("engine") != engine:
                    failures.append("engine identity differs")
                if receipt.get("ok") is not True:
                    failures.append("script failed")
                elif canonical(receipt.get("value")) != canonical(case["expected"]):
                    failures.append("literal expected value differs")
            except (ValueError, TypeError) as error:
                failures.append(f"invalid receipt: {error}")
    except subprocess.TimeoutExpired:
        failures.append(f"runner watchdog expired after {WATCHDOG_SECONDS}s; no cleanup claim")
        result["watchdog"] = True
    except OSError as error:
        failures.append(f"runner failed to start: {error}")
    with session.condition:
        drained = session.condition.wait_for(
            lambda: session.active == 0, timeout=max(0, deadline - time.monotonic()))
    if not drained:
        failures.append("server handler did not finish within case watchdog")
    observed = session.snapshot()
    if observed["paths"] != case["expected_paths"]:
        failures.append("independent server request counts differ")
    if observed["errors"]:
        failures.append("server fixture failed")
    result.update(passed=not failures, failures=failures, receipt=receipt, server=observed)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    executables = [(args.baseline.resolve(), "deno_core 0.399.0 / v8 147.4.0"),
                   (args.candidate.resolve(), "direct v8 147.4.0")]
    identities = [{"engine": engine, "executable": str(path),
                   "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                  for path, engine in executables]
    with Server() as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
        thread.start()
        try:
            origin = f"http://127.0.0.1:{server.server_port}"
            rows = []
            for case in cases_for(origin):
                results = [run(path, engine, case, server) for path, engine in executables]
                rows.append({**case, "source_sha256": hashlib.sha256(case["source"].encode()).hexdigest(),
                             "results": results, "passed": all(r["passed"] for r in results)})
                print(case["name"], "PASS" if rows[-1]["passed"] else "FAIL", flush=True)
            report = {"harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                      "origin": origin, "watchdog_seconds": WATCHDOG_SECONDS,
                      "executables": identities, "rows": rows,
                      "passed": all(row["passed"] for row in rows),
                      "limits": "No live-host idle cancellation/socket-closure claim; request counters observe parsed HTTP requests."}
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            return 0 if report["passed"] else 1
        finally:
            server.shutdown()
            thread.join(timeout=1)


if __name__ == "__main__":
    sys.exit(main())
