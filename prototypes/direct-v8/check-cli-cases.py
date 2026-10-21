#!/usr/bin/env python3
"""Actual CLI interoperability cases for #4302: one row per case, every case runs.

Each case builds or serves its own copy of a fixture with the given binary, so a
failure in one case never hides another. `--rolldown` runs every case with
ZFB_ROLLDOWN_PROTOTYPE=1; without it an inherited value is removed. A failing row
is a compatibility result to record, not something to retry. The fixture copies
live in a temporary directory; the report and per-case logs are kept.

Cases (fixture: site/ plus small additions written here):
- build-islands-css: island marker, CSS module class and an emitted islands asset
- build-mdx-content: collection route rendered from MDX
- build-wasm-ssr: SSR page instantiates an imported Wasm module
- build-wasm-copied: the Wasm module is copied next to the SSR bundle
- build-source-map: the SSR bundle has a linked map naming an authored page
- build-source-map-positions: authored text in the SSR bundle maps back to the
  authored file and to a line inside the JSX element that holds it
- browser-hydration: Chromium loads the built site, the Counter island renders,
  its CSS module colour applies and a click increments it, with no page or
  console errors (needs `--chromium`; recorded as NOT RUN without it)
- build-tsconfig-alias: a page imports through a tsconfig `paths` alias
- build-plugin-virtual-module: a config plugin's `addVirtualModule` value reaches
  both the SSR page and a client script that import it
- build-client-script: a `*.client.ts` entry outside any island is emitted under
  `assets/client/` with a hash and the page's `clientScript()` URL is rewritten to it
- browser-client-script: Chromium runs that client script, which writes the
  virtual module's value into the page (needs `--chromium`)
- build-module-worker: an island's `new URL(..., import.meta.url)` worker is emitted
  and referenced with its version query
- build-missing-export-rejected: an import of a missing export fails the build and
  names the symbol (negative control)
- dev-ssr-request: `zfb dev` renders a `prerender = false` page per request, and
  serves the MDX and Wasm routes

`--summary REPORT` prints an existing report as Markdown for a CI job summary.
"""
import argparse
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

HERE = Path(__file__).resolve().parent
SITE = HERE / "site"
_spec = importlib.util.spec_from_file_location("check_cli_lifecycle", HERE / "check-cli-lifecycle.py")
lifecycle = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(lifecycle)

ALIAS_FILES = {
    "tsconfig.json": json.dumps({"compilerOptions": {"baseUrl": ".", "paths": {"@case-lib/*": ["case-lib/*"]}}}),
    "case-lib/answer.ts": 'export const aliasAnswer = "CASE_ALIAS_" + (40 + 2);\n',
    "pages/alias.tsx": '''import { aliasAnswer } from "@case-lib/answer";

export default function AliasPage() {
  return <html><body><p data-case="alias">{aliasAnswer}</p></body></html>;
}
''',
}
WORKER_FILES = {
    "components/case.worker.ts": 'self.postMessage("CASE_WORKER_BODY");\n',
    "components/worker-button.tsx": '''"use client";

export function WorkerButton() {
  return (
    <button type="button" on:click={() => new Worker(new URL("./case.worker.ts", import.meta.url), { type: "module" })}>
      CASE_WORKER_BUTTON
    </button>
  );
}
''',
    "pages/worker.tsx": '''import { Island } from "@takazudo/zfb";

import { WorkerButton } from "../components/worker-button";

export default function WorkerPage() {
  return (
    <html><body>
      <Island when="load"><WorkerButton /></Island>
    </body></html>
  );
}
''',
}
PLUGIN_FILES = {
    "zfb.config.ts": 'export default { outDir: "dist", plugins: [{ name: "./case-plugin.mjs" }] };\n',
    "case-plugin.mjs": '''export default {
  name: "zfb-cli-case-plugin",
  setup({ addVirtualModule }) {
    addVirtualModule("virtual:case-data", () => `export const caseValue = "CASE_VIRTUAL_" + (40 + 2);`);
  },
};
''',
    "components/case.client.ts": '''import { caseValue } from "virtual:case-data";

const out = document.getElementById("case-client-out");
if (out) out.textContent = "CASE_CLIENT_RAN:" + caseValue;
''',
    "pages/index.tsx": '''import { clientScript } from "@takazudo/zfb";
import { caseValue } from "virtual:case-data";

export default function PluginPage() {
  return (
    <html><body>
      <p data-case="virtual">{caseValue}</p>
      <p id="case-client-out">CASE_CLIENT_PENDING</p>
      <script type="module" src={clientScript("case")}></script>
    </body></html>
  );
}
''',
}
MISSING_EXPORT_FILES = {
    "case-lib/answer.ts": "export const present = 1;\n",
    "pages/index.tsx": '''import { caseMissingExport } from "../case-lib/answer";

export default function Page() {
  return <html><body>{String(caseMissingExport)}</body></html>;
}
''',
}
SSR_PAGE = '''export const prerender = false;

export default function SsrPage() {
  const token = Math.random().toString(36).slice(2, 12);
  return <html><body><p data-case="ssr">CASE_SSR_RENDER_{token}</p></body></html>;
}
'''
# Authored text the SSR bundle carries, and the JSX element that holds it.
SOURCE_MAP_MARKERS = [("pages/index.tsx", "CSS module marker", "<p ", "</p>"),
                      ("components/counter.tsx", "Count: ", "<button", "</button>")]
B64 = {char: index for index, char in enumerate("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/")}
# esbuild says "No matching export", Rolldown MISSING_EXPORT / "is not exported".
MISSING_EXPORT_DIAGNOSTICS = ["No matching export", "MISSING_EXPORT", "is not exported"]


def text(path):
    return path.read_text(errors="replace") if path.is_file() else ""


def fixture(root, *, base=True, extra=None):
    if base:
        shutil.copytree(SITE, root, ignore=shutil.ignore_patterns("expected.json"))
    else:
        (root / "pages").mkdir(parents=True)
        (root / "zfb.config.ts").write_text("export default { outDir: 'dist' };\n")
    for relative, content in (extra or {}).items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    return root


def vlq(segment):
    values, value, shift = [], 0, 0
    for char in segment:
        digit = B64[char]
        value |= (digit & 31) << shift
        if digit & 32:
            shift += 5
        else:
            values.append(-(value >> 1) if value & 1 else value >> 1)
            value = shift = 0
    return values


def decode_mappings(mappings):
    """Per generated line, the (column, source index, 0-based line, column) segments."""
    lines, source, line, column = [], 0, 0, 0
    for generated in mappings.split(";"):
        segments, generated_column = [], 0
        for segment in filter(None, generated.split(",")):
            fields = vlq(segment)
            generated_column += fields[0]
            if len(fields) >= 4:
                source, line, column = source + fields[1], line + fields[2], column + fields[3]
                segments.append((generated_column, source, line, column))
        lines.append(segments)
    return lines


def original_position(source_map, generated_line, generated_column):
    """The original (source, 1-based line) of the last mapped segment at or before the position."""
    segments = decode_mappings(source_map["mappings"])
    candidates = [seg for seg in segments[generated_line] if seg[0] <= generated_column] \
        if generated_line < len(segments) else []
    if not candidates:
        return None
    _, source, line, _ = candidates[-1]
    return source_map["sources"][source].replace("\\", "/"), line + 1


def element_span(authored, marker, open_tag, close_tag):
    lines = authored.splitlines()
    at = next(index for index, line in enumerate(lines) if marker in line)
    start = next(index for index in range(at, -1, -1) if open_tag in lines[index])
    end = next(index for index in range(at, len(lines)) if close_tag in lines[index])
    return start + 1, end + 1


def build(root, logs, label, env):
    with (logs / f"{label}.stdout").open("wb") as out, (logs / f"{label}.stderr").open("wb") as err:
        child = lifecycle.spawn([str(BINARY), "build"], cwd=root, env=env, stdout=out, stderr=err)
        try:
            return child.wait(timeout=300)
        except subprocess.TimeoutExpired:
            lifecycle.kill_tree(child)
            child.wait()
            raise AssertionError(f"{label}: build exceeded 300 s (harness limit)")


def dist_files(root, suffix):
    return sorted(path for path in (root / "dist").rglob(f"*{suffix}") if path.is_file())


def html(root, route):
    candidates = [root / "dist" / route / "index.html", root / "dist" / f"{route}.html"]
    return next((text(path) for path in candidates if path.is_file()), "")


class Rows:
    def __init__(self, report):
        self.report = report

    def record(self, name, check):
        try:
            detail = check()
            self.report["rows"].append({"name": name, "passed": True, **(detail or {})})
        except Exception as error:
            self.report["rows"].append({"name": name, "passed": False, "error": f"{type(error).__name__}: {error}"})


def site_build_rows(rows, work, logs, env):
    root = fixture(work / "site")
    code = build(root, logs, "site-build", env)
    built = code == 0

    def require_build():
        assert built, f"site build exit {code} (see site-build.stderr)"

    def islands_css():
        require_build()
        page = html(root, "")
        for marker in ['data-v8-fixture="home"', 'data-zfb-island="Counter"', "CSS module marker"]:
            assert marker in page, f"index.html lacks {marker}"
        tag = re.search(r'<p[^>]*data-v8-fixture="css-module"[^>]*>', page)
        css_class = tag and re.search(r'class="([^"\s]+)', tag[0])
        assert css_class, "CSS module marker has no class"
        assert any(css_class[1] in text(path) for path in dist_files(root, ".css")), \
            f"no emitted stylesheet defines {css_class[1]}"
        scripts = [path for path in dist_files(root, ".js") if "Count: " in text(path)]
        assert scripts, "no emitted script carries the Counter island body"
        return {"islands_asset": scripts[0].relative_to(root).as_posix()}

    def mdx():
        require_build()
        page = html(root, "posts/hello")
        for marker in ["MDX content marker", 'data-v8-fixture="post"']:
            assert marker in page, f"posts/hello lacks {marker}"

    def wasm_ssr():
        require_build()
        assert "Wasm answer: 42" in html(root, "wasm"), "wasm page lacks the instantiated answer"

    def wasm_copied():
        require_build()
        bundle = root / ".zfb-build/bundle.mjs"
        copies = sorted(path for path in bundle.parent.glob("*.wasm") if path.name in text(bundle))
        assert copies, "no .wasm beside bundle.mjs that the bundle references"
        return {"copies": [path.relative_to(root).as_posix() for path in copies]}

    def source_map():
        require_build()
        bundle_map = root / ".zfb-build/bundle.mjs.map"
        assert bundle_map.is_file(), "no .zfb-build/bundle.mjs.map"
        sources = json.loads(bundle_map.read_text()).get("sources", [])
        assert any(source.replace("\\", "/").endswith("pages/index.tsx") for source in sources), \
            "map sources name no authored pages/index.tsx"
        return {"source_count": len(sources)}

    def source_map_positions():
        require_build()
        bundle = root / ".zfb-build/bundle.mjs"
        source_map = json.loads((root / ".zfb-build/bundle.mjs.map").read_text())
        generated = text(bundle).split("\n")
        positions = []
        for authored_path, marker, open_tag, close_tag in SOURCE_MAP_MARKERS:
            line = next((index for index, code in enumerate(generated) if marker in code), None)
            assert line is not None, f"bundle.mjs lacks {marker!r}"
            mapped = original_position(source_map, line, generated[line].index(marker))
            assert mapped, f"{marker!r} has no mapping"
            source, original_line = mapped
            span = element_span(text(root / authored_path), marker, open_tag, close_tag)
            positions.append({"marker": marker, "source": source, "line": original_line, "element_lines": span})
            assert source.endswith(authored_path), f"{marker!r} maps to {source}, not {authored_path}"
            assert span[0] <= original_line <= span[1], \
                f"{marker!r} maps to {authored_path}:{original_line}, outside its element (lines {span[0]}-{span[1]})"
        return {"positions": positions}

    def hydration():
        require_build()
        return browser_hydration(root, logs)

    rows.record("build-islands-css", islands_css)
    rows.record("build-mdx-content", mdx)
    rows.record("build-wasm-ssr", wasm_ssr)
    rows.record("build-wasm-copied", wasm_copied)
    rows.record("build-source-map", source_map)
    rows.record("build-source-map-positions", source_map_positions)
    if CHROMIUM:
        rows.record("browser-hydration", hydration)
    else:
        rows.report["not_run"].append({"name": "browser-hydration", "reason": "no --chromium given"})


@contextlib.contextmanager
def serve_dist(root):
    """Serve dist/ over HTTP; yields the origin and the list of paths that 404."""
    missing = []

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            # Chromium asks for a favicon the fixtures do not ship, as check-generated.py does.
            if self.path == "/favicon.ico":
                self.send_response(204)
                self.end_headers()
                return
            if not (root / "dist" / self.path.split("?")[0].lstrip("/")).exists():
                missing.append(self.path)
            super().do_GET()

    with ThreadingHTTPServer(("127.0.0.1", 0), partial(Handler, directory=str(root / "dist"))) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}", missing
        finally:
            server.shutdown()
            thread.join(timeout=1)


def browser_run(logs, label, script, args, missing):
    node = shutil.which("node")
    assert node, "node is not on PATH"
    output = logs / f"{label}.json"
    # Repository root, where @playwright/test resolves.
    process = subprocess.run([node, str(HERE / script), *args, str(CHROMIUM), str(output)], cwd=HERE.parents[1],
                             capture_output=True, text=True, timeout=60, check=False)
    (logs / f"{label}.stdout").write_text(process.stdout)
    (logs / f"{label}.stderr").write_text(process.stderr)
    report = json.loads(output.read_text()) if output.is_file() else {}
    assert process.returncode == 0 and report.get("passed") is True, \
        f"{script} exit {process.returncode} (404s {missing}): " \
        f"{(report.get('error') or process.stderr).splitlines()[:1]}"
    return report


def browser_hydration(root, logs):
    """Drive the Counter island of the built site with browser-check.mjs."""
    with serve_dist(root) as (origin, missing):
        report = browser_run(logs, "browser-hydration", "browser-check.mjs",
                             [origin, str(SITE / "expected.json")], missing)
    return {"chromium": report.get("version"), "after_click": report.get("afterClickText")}


def browser_text(root, logs, label, route, selector, expected):
    with serve_dist(root) as (origin, missing):
        return browser_run(logs, label, "browser-text.mjs", [origin + route, selector, expected], missing)


def alias_row(rows, work, logs, env):
    def check():
        root = fixture(work / "alias", base=False, extra=ALIAS_FILES)
        code = build(root, logs, "alias-build", env)
        assert code == 0, f"alias build exit {code}"
        assert "CASE_ALIAS_42" in html(root, "alias"), "alias page lacks the aliased value"
    rows.record("build-tsconfig-alias", check)


def plugin_client_rows(rows, work, logs, env):
    root = fixture(work / "plugin", base=False, extra=PLUGIN_FILES)
    code = build(root, logs, "plugin-build", env)

    def client_scripts():
        assert code == 0, f"plugin build exit {code} (see plugin-build.stderr)"
        found = sorted((root / "dist/assets/client").glob("case-*.js"))
        assert len(found) == 1, f"expected one hashed assets/client/case-*.js, found {len(found)}"
        return found[0]

    def virtual_module():
        script = client_scripts()
        assert "CASE_VIRTUAL_42" in html(root, ""), "SSR page lacks the virtual module value"
        assert "CASE_VIRTUAL_" in text(script), "client script lacks the virtual module value"

    def client_script():
        script = client_scripts()
        page = html(root, "")
        url = "/" + script.relative_to(root / "dist").as_posix()
        assert f'src="{url}"' in page, f"page does not reference {url}"
        assert "/assets/client/case.js" not in page, "page still carries the unhashed clientScript() URL"
        assert "CASE_CLIENT_RAN:" in text(script), "client script body missing"
        return {"script": url}

    def browser_client_script():
        client_scripts()
        report = browser_text(root, logs, "browser-client-script", "/", "#case-client-out",
                              "CASE_CLIENT_RAN:CASE_VIRTUAL_42")
        return {"chromium": report.get("version")}

    rows.record("build-plugin-virtual-module", virtual_module)
    rows.record("build-client-script", client_script)
    if CHROMIUM:
        rows.record("browser-client-script", browser_client_script)
    else:
        rows.report["not_run"].append({"name": "browser-client-script", "reason": "no --chromium given"})


def worker_row(rows, work, logs, env):
    def check():
        root = fixture(work / "worker", base=False, extra=WORKER_FILES)
        code = build(root, logs, "worker-build", env)
        assert code == 0, f"worker build exit {code}"
        scripts = dist_files(root, ".js")
        workers = [path for path in scripts if "CASE_WORKER_BODY" in text(path)]
        assert len(workers) == 1, f"expected one emitted worker, found {len(workers)}"
        worker = workers[0]
        parents = [path for path in scripts if path != worker
                   and re.search(re.escape(worker.name) + r"\?v=[0-9a-f]{8}", text(path))]
        assert parents, f"no emitted script references {worker.name}?v=<digest>"
        return {"worker": worker.relative_to(root).as_posix(), "parent": parents[0].relative_to(root).as_posix()}
    rows.record("build-module-worker", check)


def missing_export_row(rows, work, logs, env):
    def check():
        root = fixture(work / "missing-export", base=False, extra=MISSING_EXPORT_FILES)
        code = build(root, logs, "missing-export-build", env)
        assert code != 0, "build with a missing export succeeded"
        output = text(logs / "missing-export-build.stdout") + text(logs / "missing-export-build.stderr")
        assert "caseMissingExport" in output, "diagnostic does not name the missing export"
        assert any(marker in output for marker in MISSING_EXPORT_DIAGNOSTICS), \
            "build failed without a missing-export diagnostic"
        return {"exit": code}
    rows.record("build-missing-export-rejected", check)


def dev_row(rows, work, logs, env):
    def check():
        root = fixture(work / "dev", extra={"pages/ssr.tsx": SSR_PAGE})
        stdout, stderr = logs / "dev.stdout", logs / "dev.stderr"
        dev_env = {key: value for key, value in env.items() if not key.startswith("ZFB_DEV_")}
        with stdout.open("wb") as out, stderr.open("wb") as err:
            child = lifecycle.spawn([str(BINARY), "dev", "--port", "0"], cwd=root, env=dev_env,
                                    stdout=out, stderr=err, interruptible=True)
        try:
            deadline = time.monotonic() + 90
            while not (match := re.search(r"listening on \S+:(\d+)", text(stdout) + text(stderr))):
                assert child.poll() is None, f"dev exited before listening: {child.returncode}"
                assert time.monotonic() < deadline, "deadline waiting for dev listener"
                time.sleep(0.05)
            base = f"http://127.0.0.1:{match[1]}"

            def get(route):
                deadline = time.monotonic() + 60
                while True:
                    try:
                        with urllib.request.urlopen(base + route, timeout=5) as response:
                            if response.status == 200:
                                return response.read().decode(errors="replace")
                    except OSError:
                        pass
                    assert child.poll() is None, f"dev exited while serving {route}: {child.returncode}"
                    assert time.monotonic() < deadline, f"deadline waiting for 200 on {route}"
                    time.sleep(0.2)

            first = re.search(r"CASE_SSR_RENDER_(\w+)", get("/ssr"))
            second = re.search(r"CASE_SSR_RENDER_(\w+)", get("/ssr"))
            assert first and second, "SSR page marker missing"
            assert first[1] != second[1], "prerender = false page served the same render twice"
            assert "MDX content marker" in get("/posts/hello"), "dev MDX route lacks its content"
            assert "Wasm answer: 42" in get("/wasm"), "dev Wasm route lacks the instantiated answer"
            lifecycle.interrupt(child)
            code = child.wait(timeout=30)
            assert code == 0, f"dev shutdown exit {code}"
        finally:
            if child.poll() is None:
                lifecycle.kill_tree(child)
                child.wait()
    rows.record("dev-ssr-request", check)


def summary(report):
    lines = [f"CLI cases ({'Rolldown' if report['rolldown_prototype'] else 'default bundler'}): "
             + ("PASS" if report["passed"] else "FAIL")]
    lines += [f"- {row['name']}: {row['passed']} {row.get('error', '')}".rstrip() for row in report["rows"]]
    lines += [f"- {row['name']}: NOT RUN ({row['reason']})" for row in report.get("not_run", [])]
    return "\n".join(lines)


def main():
    global BINARY, CHROMIUM
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path, help="New directory for the report and logs")
    parser.add_argument("--rolldown", action="store_true", help="Run every case with ZFB_ROLLDOWN_PROTOTYPE=1")
    parser.add_argument("--chromium", type=Path, help="Installed Chromium for the hydration row; never downloaded")
    parser.add_argument("--summary", type=Path, metavar="REPORT", help="Print REPORT as Markdown and exit")
    args = parser.parse_args()
    if args.summary:
        print(summary(json.loads(args.summary.read_text())))
        return 0
    if not args.binary or not args.output:
        parser.error("--binary and --output are required")
    BINARY = args.binary.resolve()
    CHROMIUM = args.chromium.resolve() if args.chromium else None
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    logs = output / "logs"
    logs.mkdir()
    env = {key: value for key, value in os.environ.items() if key != "ZFB_ROLLDOWN_PROTOTYPE"}
    if args.rolldown:
        env["ZFB_ROLLDOWN_PROTOTYPE"] = "1"
    report = {"binary": str(BINARY), "sha256": lifecycle.digest(BINARY),
              "rolldown_prototype": env.get("ZFB_ROLLDOWN_PROTOTYPE"), "rows": [], "not_run": []}
    rows = Rows(report)
    with tempfile.TemporaryDirectory(prefix="zfb-cli-cases-") as directory:
        work = Path(directory)
        site_build_rows(rows, work, logs, env)
        alias_row(rows, work, logs, env)
        plugin_client_rows(rows, work, logs, env)
        worker_row(rows, work, logs, env)
        missing_export_row(rows, work, logs, env)
        dev_row(rows, work, logs, env)
    report["binary_unchanged"] = lifecycle.digest(BINARY) == report["sha256"]
    report["passed"] = report["binary_unchanged"] and all(row["passed"] for row in report["rows"])
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
