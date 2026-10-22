#!/usr/bin/env python3
"""Consume one frozen #4240 snapshot in independent native runners.

No producer runs here. Canonical Rust helpers postprocess candidate HTML and
map error frames. Node only drives one explicit, already-installed Chromium.
No browser download, runtime fallback, arbitrary normalization or CLI adoption.
"""
import argparse
from functools import partial
import hashlib
from html.parser import HTMLParser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading

ROOT = Path(__file__).resolve().parents[2]
HOST_PREFIX = "ZFB_HOST_RECEIPT "
POST_PREFIX = "ZFB_POSTPROCESS_RECEIPT "
# The baseline is the deno_core rollback runner. direct-v8 is the default since
# #4390, so build it with the pinned rollback features or it exits 2.
BASELINE_ENGINE = "deno_core 0.399.0 / v8 147.4.0"
BASELINE_BUILD_FEATURES = "--no-default-features --features embed_v8,syntect-onig"
BASELINE_HELP = ("direct_v8_baseline example built with `cargo build --locked -p zfb-render "
                 + BASELINE_BUILD_FEATURES + " --example direct_v8_baseline`")
CANDIDATE_ENGINE = "direct v8 147.4.0"


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def equal(left, right):
    return json.dumps(left, sort_keys=True) == json.dumps(right, sort_keys=True)


def contained(root, name):
    target = (root / name).resolve()
    if not target.is_relative_to(root):
        raise ValueError("snapshot path escapes root: " + name)
    return target


def verify_snapshot(snapshot, manifest):
    if any(path.is_symlink() for path in snapshot.rglob("*")):
        raise ValueError("snapshot gained a symlink")
    actual = {p.relative_to(snapshot).as_posix() for p in snapshot.rglob("*") if p.is_file() and p != snapshot / "manifest.json"}
    if actual != set(manifest["files"]):
        raise ValueError("snapshot file inventory changed")
    for name, expected in manifest["files"].items():
        path = contained(snapshot, name)
        if path.is_symlink() or path.stat().st_size != expected["bytes"] or sha(path) != expected["sha256"]:
            raise ValueError("snapshot bytes changed: " + name)


class Document(HTMLParser):
    """Small HTML assertion tree; supports only this fixture's literal selectors."""
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}

    def __init__(self, html):
        super().__init__(convert_charrefs=True)
        self.nodes = []
        self.stack = []
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        node = {"tag": tag, "attrs": dict(attrs), "text": "", "ancestors": list(self.stack)}
        self.nodes.append(node)
        if tag not in self.VOID:
            self.stack.append(node)

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if self.stack and self.stack[-1]["tag"] == tag:
            self.stack.pop()

    def handle_endtag(self, tag):
        for i in range(len(self.stack) - 1, -1, -1):
            if self.stack[i]["tag"] == tag:
                del self.stack[i:]
                break

    def handle_data(self, data):
        for node in self.stack:
            node["text"] += data

    @staticmethod
    def matches(node, selector):
        if re.fullmatch(r"[a-z][a-z0-9-]*", selector):
            return node["tag"] == selector
        attribute = re.fullmatch(r'\[([a-z0-9-]+)="([^"]*)"\]', selector)
        if attribute:
            return node["attrs"].get(attribute[1]) == attribute[2]
        raise ValueError("unsupported authored selector: " + selector)

    def select(self, selector):
        parts = selector.split(" ")
        if len(parts) == 1:
            return [node for node in self.nodes if self.matches(node, selector)]
        if len(parts) == 2:
            return [node for node in self.nodes if self.matches(node, parts[1]) and
                    any(self.matches(ancestor, parts[0]) for ancestor in node["ancestors"])]
        raise ValueError("unsupported authored selector: " + selector)


def route_failures(response, expected):
    failures = []
    for field in ("status", "headers", "body"):
        if field in expected and not equal(response.get(field), expected[field]):
            failures.append(field + " differs from literal expectation")
    if "html" in expected:
        text = bytes(response["body"]).decode("utf8", errors="strict")
        document = Document(text)
        for marker in expected["html"].get("contains", []):
            if marker not in text:
                failures.append("missing HTML marker: " + marker)
        for marker in expected["html"].get("excludes", []):
            if marker in text:
                failures.append("forbidden HTML marker: " + marker)
        for assertion in expected["html"].get("text", []):
            selected = document.select(assertion["selector"])
            # Whitespace collapsing applies only to DOM text assertions, never
            # response bytes, headers, JSON values or source-map coordinates.
            values = [" ".join(node["text"].split()) for node in selected]
            if values != [assertion["equals"]]:
                failures.append("selector text differs: " + assertion["selector"])
    if "json_length" in expected or "json_assertions" in expected:
        value = json.loads(bytes(response["body"]))
        if "json_length" in expected and len(value) != expected["json_length"]:
            failures.append("JSON length differs")
        for assertion in expected.get("json_assertions", []):
            actual = value
            try:
                for part in assertion["pointer"].split("/")[1:]:
                    key = part.replace("~1", "/").replace("~0", "~")
                    actual = actual[int(key)] if isinstance(actual, list) else actual[key]
                if not equal(actual, assertion["equals"]):
                    failures.append("JSON pointer differs: " + assertion["pointer"])
            except (KeyError, IndexError, ValueError, TypeError):
                failures.append("missing JSON pointer: " + assertion["pointer"])
    return failures


def invoke(binary, requests, prefix, output, label, timeout=45):
    wire = "".join(json.dumps(request) + "\n" for request in requests)
    (output / f"{label}.input.jsonl").write_text(wire)
    try:
        process = subprocess.run([str(binary)], input=wire, text=True, capture_output=True,
                                 timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        for channel in ("stdout", "stderr"):
            partial_output = getattr(error, channel) or b""
            (output / f"{label}.{channel}").write_bytes(
                partial_output if isinstance(partial_output, bytes) else partial_output.encode())
        raise ValueError(f"{label}: {timeout}s watchdog failure; no clean shutdown claim") from error
    (output / f"{label}.stdout").write_text(process.stdout)
    (output / f"{label}.stderr").write_text(process.stderr)
    if process.returncode:
        raise ValueError(f"{label}: runner exit {process.returncode}")
    receipts = [json.loads(line[len(prefix):]) for line in process.stdout.splitlines() if line.startswith(prefix)]
    if len(receipts) != len(requests) or any(not isinstance(r, dict) for r in receipts):
        raise ValueError(f"{label}: expected {len(requests)} framed object receipts; got {len(receipts)}")
    return receipts


def packet(snapshot, source, requests, name="file:///zfb/bundle.mjs", **kwargs):
    return {"kind": "worker", "name": name, "source": source,
            "asset_root": str(snapshot / "main"), "requests": requests, **kwargs}


def requests_for(snapshot, manifest, expected):
    source = (snapshot / "main/bundle.mjs").read_text()
    requests = [{**route["request"], "mode": "build-time"} for route in expected["routes"]]
    main = packet(snapshot, source, requests)
    wrapper = manifest["policy"]["wrapper"]
    plugin_request = {"url": "http://zfb.local/plugin", "method": "POST", "mode": "request-time",
                      "headers": {"x-input": "request-marker"}, "body": [0, 128, 255]}
    binary_request = {"url": "http://zfb.local/binary", "mode": "request-time"}
    return [
        ("emitted-config", {"kind": "config", "source": (snapshot / "config/config.mjs").read_text()}),
        ("generated-main", main),
        ("generated-wrapper-inner", packet(snapshot, (snapshot / "wrapper/wrapper.mjs").read_text(), requests,
            name=wrapper["wrapper_name"], inner={"name": wrapper["inner_name"], "source": source})),
        ("prepared-plugin-and-binary-request", packet(snapshot, (snapshot / "plugin/worker.mjs").read_text(),
            [plugin_request, binary_request], name="file:///zfb/plugin.mjs", aliases=[{
                "specifier": "plugin:answer", "target": str(snapshot / "plugin/alias.mjs"), "plugin": "direct-v8-fixture"}],
            virtual_modules=[{"specifier": "virtual:message", "source": (snapshot / "plugin/virtual.mjs").read_text(),
                              "plugin": "direct-v8-fixture"}])),
        ("authored-mapped-throw", packet(snapshot, (snapshot / "negative/throw.mjs").read_text(),
            [{"url": "http://zfb.local/negative", "mode": "build-time"}], name="file:///zfb/throw.mjs")),
        ("fresh-after-handler-failure", main),
        ("finite-create-load-dispatch-drop-2", main),
        ("finite-create-load-dispatch-drop-3", main),
    ]


def inspect_receipts(engine, named, receipts, expected, manifest, snapshot, postprocessor, output, label):
    rows = []
    for (name, _), receipt in zip(named, receipts):
        failures = []
        if receipt.get("engine") != engine:
            failures.append("engine identity differs")
        if name == "authored-mapped-throw":
            wanted = manifest["policy"]["negative_expected"]
            if receipt.get("ok") is not False or wanted["message"] not in receipt.get("error", ""):
                failures.append("missing deliberate handler failure")
            if wanted["console"] not in receipt.get("console", ""):
                failures.append("missing handler console marker")
            positions = re.findall(r"(?:file:///zfb/)?throw\.mjs:(\d+):(\d+)", receipt.get("error", ""))
            maps = [{"kind": "map", "map_json": (snapshot / "negative/throw.mjs.map").read_text(),
                     "line": int(line), "column": int(column)} for line, column in positions]
            mapped = invoke(postprocessor, maps, POST_PREFIX, output, label + "-maps") if maps else []
            matches = [r["value"] for r in mapped if r.get("ok") is True and
                       r["value"]["file"].endswith(wanted["file_suffix"]) and
                       r["value"]["line"] == wanted["line"] and r["value"]["column"] == wanted["column"] and
                       r["value"]["source_content"] == (snapshot / "negative/throw.ts").read_text()]
            if not matches:
                failures.append("no canonical source-map frame at authored 6:9 with exact source content")
            rows.append({"name": name, "passed": not failures, "failures": failures,
                         "error": receipt.get("error"), "console": receipt.get("console"), "mapped_frames": mapped})
            continue
        if receipt.get("ok") is not True:
            failures.append("expected successful native result")
        elif name == "emitted-config":
            if not equal(receipt.get("value"), manifest["policy"]["config_expected"]):
                failures.append("pre-emitted config differs from literal expectation")
        else:
            responses = receipt.get("value", {}).get("responses", [])
            if name == "prepared-plugin-and-binary-request":
                plugin_expected = {"status": 200, "headers": [["content-type", "application/json"],
                    ["set-cookie", "plugin=one"], ["set-cookie", "plugin=two"], ["x-prepared", "42"]],
                    "json_assertions": [{"pointer": "/" + key, "equals": value} for key, value in {
                        "answer": 42, "message": "virtual-ok", "method": "POST", "marker": "request-marker",
                        "body": [0, 128, 255]}.items()]}
                binary_expected = {"status": 201, "body": [0, 255, 128, 65],
                    "headers": [["content-type", "application/octet-stream"],
                    ["set-cookie", "binary=one"], ["set-cookie", "binary=two"],
                    ["x-prepared", "left, right"]]}
                wanted_routes = [plugin_expected, binary_expected]
                if "prepared plugin request" not in receipt["value"].get("console", ""):
                    failures.append("missing plugin console marker")
            else:
                wanted_routes = [route["expected"] for route in expected["routes"]]
            if len(responses) != len(wanted_routes):
                failures.append("response count differs")
            for index, (response, wanted) in enumerate(zip(responses, wanted_routes)):
                try:
                    failures += [f"route {index}: {failure}" for failure in route_failures(response, wanted)]
                except Exception as error:
                    failures.append(f"route {index}: invalid response: {error}")
        rows.append({"name": name, "passed": not failures, "failures": failures,
                     "receipt_sha256": hashlib.sha256(json.dumps(receipt, sort_keys=True).encode()).hexdigest(),
                     "error": receipt.get("error")})
    return rows


def browser_check(snapshot, html, expected, postprocessor, node, chromium, output):
    generated = Document((snapshot / "dist/index.html").read_text())
    css = [n["attrs"]["href"] for n in generated.nodes if n["tag"] == "link" and n["attrs"].get("rel") == "stylesheet"]
    modules = [n["attrs"]["src"] for n in generated.nodes if n["tag"] == "script" and n["attrs"].get("type") == "module" and "src" in n["attrs"]]
    if len(css) != 1 or not modules:
        raise ValueError("expected one emitted stylesheet and at least one island module")
    for url in css + modules:
        if not url.startswith("/") or not contained(snapshot / "dist", url[1:]).is_file():
            raise ValueError("emitted browser asset missing or nonlocal: " + url)
    response = invoke(postprocessor, [{"kind": "html", "html": html, "css_url": css[0],
                       "island_module_urls": modules}], POST_PREFIX, output, "html-postprocess")[0]
    if response.get("ok") is not True or not isinstance(response.get("value"), str):
        raise ValueError("canonical HTML postprocessor failed")
    dist = output / "browser-dist"
    shutil.copytree(snapshot / "dist", dist)
    (dist / "index.html").write_text(response["value"])

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            if self.path == "/favicon.ico":
                self.send_response(204)
                self.end_headers()
                return
            super().do_GET()

    with ThreadingHTTPServer(("127.0.0.1", 0), partial(Handler, directory=str(dist))) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        try:
            origin = f"http://127.0.0.1:{server.server_port}"
            command = [str(node), str(Path(__file__).with_name("browser-check.mjs")), origin,
                       str(snapshot / "expected.json"), str(chromium), str(output / "browser.json")]
            process = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=45, check=False)
            (output / "browser.stdout").write_text(process.stdout)
            (output / "browser.stderr").write_text(process.stderr)
            report = json.loads((output / "browser.json").read_text()) if (output / "browser.json").exists() else {"passed": False}
            report.update(exit_code=process.returncode, command=command, executable_sha256=sha(chromium),
                          candidate_html_sha256=sha(dist / "index.html"), stylesheet=css, island_modules=modules)
            report["passed"] = process.returncode == 0 and report.get("passed") is True
            return report
        finally:
            server.shutdown()
            thread.join(timeout=1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("snapshot", "baseline", "candidate", "postprocessor", "node", "chromium", "output"):
        parser.add_argument("--" + name, type=Path, required=True,
                            help=BASELINE_HELP if name == "baseline" else None)
    args = parser.parse_args()
    for name, value in vars(args).items():
        setattr(args, name, value.resolve())
    if not args.output.is_relative_to(ROOT / "target") or args.output.exists():
        parser.error("--output must be a new directory below repository target/")
    args.output.mkdir(parents=True)
    report = {"passed": False, "limitations": ["No candidate CLI/dev reload integration or live-idle cancellation claim.",
                "Watchdogs are failures; installed Chromium is explicit and no browser download is attempted."]}
    try:
        manifest = json.loads((args.snapshot / "manifest.json").read_text())
        verify_snapshot(args.snapshot, manifest)
        expected = json.loads((args.snapshot / "expected.json").read_text())
        named = requests_for(args.snapshot, manifest, expected)
        report["snapshot_manifest_sha256"] = sha(args.snapshot / "manifest.json")
        report["harness_sha256"] = {name: sha(Path(__file__).with_name(name)) for name in
                                    ("prepare-fixtures.py", "check-generated.py", "browser-check.mjs")}
        report["executables"] = {name: {"path": str(getattr(args, name)), "sha256": sha(getattr(args, name))}
                                 for name in ("baseline", "candidate", "postprocessor", "node", "chromium")}
        report["input_sha256"] = hashlib.sha256(json.dumps([p for _, p in named], sort_keys=True).encode()).hexdigest()
        observations = []
        for label, binary, engine in (("baseline", args.baseline, BASELINE_ENGINE),
                                      ("candidate", args.candidate, CANDIDATE_ENGINE)):
            receipts = invoke(binary, [packet for _, packet in named], HOST_PREFIX, args.output, label)
            rows = inspect_receipts(engine, named, receipts, expected, manifest, args.snapshot,
                                    args.postprocessor, args.output, label)
            report[label] = {"engine": engine, "passed": all(r["passed"] for r in rows), "rows": rows}
            observations.append(receipts)
        parity = []
        for index, (name, _) in enumerate(named):
            if name == "authored-mapped-throw":
                continue  # independent exact authored mapping assertions above
            parity.append({"name": name, "passed": equal(observations[0][index].get("value"),
                                                         observations[1][index].get("value"))})
        report["byte_and_value_parity"] = parity
        if report["baseline"]["passed"] and report["candidate"]["passed"] and all(r["passed"] for r in parity):
            html = bytes(observations[1][1]["value"]["responses"][0]["body"]).decode("utf8")
            report["browser"] = browser_check(args.snapshot, html, expected, args.postprocessor,
                                                args.node, args.chromium, args.output)
        else:
            report["browser"] = {"passed": False, "not_run": "runtime assertions failed"}
        verify_snapshot(args.snapshot, manifest)
        report["passed"] = (report["baseline"]["passed"] and report["candidate"]["passed"] and
                            all(r["passed"] for r in parity) and report["browser"]["passed"])
    except Exception as error:
        report["error"] = str(error)
        report["passed"] = False
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Generated-input gate", "PASS" if report["passed"] else "FAIL", args.output / "results.json")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
