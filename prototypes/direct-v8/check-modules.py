#!/usr/bin/env python3
"""Temporary module/Worker acceptance evidence, with independent literal oracles.

Both native processes receive identical JSON requests and filesystem fixtures.
Python prepares data and compares receipts; it never executes JavaScript.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixtures(root):
    root.mkdir(parents=True, exist_ok=False)
    sources = {
        "canonical/entry.js": "import {marker} from './sibling space.js'; globalThis.aliasCount=(globalThis.aliasCount||0)+1; export {marker};",
        "canonical/sibling space.js": "export const marker=42;",
        "canonical/missing-user.js": "import './absent.js'; export const marker=1;",
        "canonical/sub/deep.js": "export const secret=123;",
        "canonical/escape-user.js": "import {secret} from './escape.js'; export {secret};",
        "outside.js": "export const secret=999;",
        "canonical/typed.ts": "export const marker: number = 42;",
        "bundle/broken.wasm": "not wasm",
    }
    for name, source in sources.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
    (root / "spelling").mkdir()
    (root / "spelling/entry.js").symlink_to(root / "canonical/entry.js")
    (root / "canonical/escape.js").symlink_to(root / "outside.js")
    wasm = Path(__file__).with_name("site") / "wasm/answer.wasm"
    (root / "bundle/answer space.wasm").write_bytes(wasm.read_bytes())
    (root / "bundle/escape.wasm").symlink_to(root / "outside.js")
    return [{"path": str(p.relative_to(root)), "bytes": p.stat().st_size,
             "sha256": digest(p), "symlink": str(p.readlink()) if p.is_symlink() else None}
            for p in sorted(root.rglob("*")) if p.is_file()]


def cases(root):
    result = []
    def add(name, source, expected=None, error=None, **kw):
        request = {"kind": "worker", "name": "file:///zfb/bundle.mjs", "source": source,
                   "requests": [{"url": "http://zfb.local/"}], **kw}
        result.append({"name": name, "request": request,
                       "expected": {"ok": error is None, "json": expected, "error": error}})
    def worker(expr):
        return "export default {async fetch(req){return Response.json(" + expr + ");}}"
    identity = "globalThis.moduleCount=(globalThis.moduleCount||0)+1; export const marker=42;"
    add("static-dynamic-namespace-evaluation", "import * as s from './identity.mjs'; const a=await import('./identity.mjs'); const b=await import('./identity.mjs'); " + worker("[a===b,a===s,moduleCount,a.marker]"), [True, True, 1, 42], modules=[{"specifier": "file:///zfb/identity.mjs", "source": identity}])
    add("top-level-await", "const value=await Promise.resolve(42);" + worker("value"), 42)
    add("import-meta", worker("({url:import.meta.url,main:import.meta.main,filename:import.meta.filename,dirname:import.meta.dirname,keys:Object.keys(import.meta).sort(),resolved:import.meta.resolve('./identity.mjs')})"), {"url": "file:///zfb/bundle.mjs", "main": True, "filename": "/zfb/bundle.mjs", "dirname": "/zfb", "keys": ["dirname", "filename", "main", "resolve", "url"], "resolved": "file:///zfb/identity.mjs"})
    add("dynamic-unknown", "await import('not-registered');" + worker("42"), error=["not-registered", "failed to resolve"])
    add("missing-export", "import {absent} from './identity.mjs';" + worker("absent"), error=["absent"], modules=[{"specifier": "file:///zfb/identity.mjs", "source": identity}])
    for name, source in [("missing", "export const x=42;"), ("undefined", "export default undefined;"), ("null", "export default null;"), ("number", "export default 42;"), ("fetch-missing", "export default {};"), ("fetch-not-callable", "export default {fetch:42};")]:
        add("strict-" + name, source, error=["default"] if not name.startswith("fetch") else ["fetch"])
    add("boot-throw-logs", "console.warn('module-boot-sentinel'); throw new Error('module-boot-error');" + worker("42"), error=["module-boot-error"])
    result[-1]["expected"]["consoleContains"] = "module-boot-sentinel"
    add("handler-throw-logs", "export default {fetch(){console.error('handler-sentinel');throw new Error('handler-error')}}", error=["handler-error"])
    result[-1]["expected"]["consoleContains"] = "handler-sentinel"
    inner = "globalThis.innerCount=(globalThis.innerCount||0)+1; export default {fetch(){return Response.json([innerCount,import.meta.main])}}"
    add("strict-inner-wrapper-once", "import inner from './inner.mjs'; export default {fetch(req){return inner.fetch(req)}}", [1, False], inner={"name": "file:///zfb/inner.mjs", "source": inner})
    add("invalid-inner-before-wrapper", worker("42"), error=["fetch"], inner={"name": "file:///zfb/inner.mjs", "source": "export default {};"})
    add("empty-body-remains-absent", worker("[req._bodyPresent,Array.from(new Uint8Array(await req.arrayBuffer()))]"), [False, []], requests=[{"url": "http://zfb.local/", "method": "POST", "body": []}])
    add("cyclic-module-link", "import {answer} from './cycle-a.mjs';" + worker("answer()"), 42, modules=[
        {"specifier": "file:///zfb/cycle-a.mjs", "source": "import {base} from './cycle-b.mjs'; export function answer(){return base()+2;} export const marker=40;"},
        {"specifier": "file:///zfb/cycle-b.mjs", "source": "import {marker} from './cycle-a.mjs'; export function base(){return marker;}"}])
    add("dynamic-ext-error-class", worker("await import('ext:zfb_node_stubs/node_stub.js').then(()=>null,e=>e.name)"), "TypeError")
    add("request-binary-headers", "export default {async fetch(req){const bytes=new Uint8Array(await req.arrayBuffer());return new Response(bytes,{status:201,headers:[['x-method',req.method],['x-input',req.headers.get('x-input')],['set-cookie','a=1'],['set-cookie','b=2']]})}}", requests=[{"url": "http://zfb.local/", "method": "POST", "headers": {"x-input": "kept"}, "body": [0, 255, 128, 65]}])
    result[-1]["expected"].update({"raw": {"status": 201, "headers": [["set-cookie", "a=1"], ["set-cookie", "b=2"], ["x-input", "kept"], ["x-method", "POST"]], "body": [0, 255, 128, 65]}})
    add("sequential-dispatch-state", "let count=0;" + worker("++count"), requests=[{"url": "http://zfb.local/"}, {"url": "http://zfb.local/"}])
    result[-1]["expected"]["jsonResponses"] = [1, 2]
    alias = [{"specifier": "fixture-alias", "target": str(root / "spelling/entry.js"), "plugin": "fixture-plugin"}]
    canonical = (root / "canonical/entry.js").as_uri()
    add("canonical-alias-identity-space-sibling", "import * as a from 'fixture-alias'; import * as b from " + json.dumps(canonical) + "; const d=await import('fixture-alias');" + worker("[a===b,b===d,a.marker,aliasCount]"), [True, True, 42, 1], aliases=alias)
    for name, specifier in [("alias-prefix-denied", "fixture-alias/deep"), ("alias-descendant-denied", (root / "canonical/sub/deep.js").as_uri()), ("alias-upward-denied", (root / "outside.js").as_uri()), ("alias-symlink-escape-denied", (root / "canonical/escape.js").as_uri())]:
        add(name, "import " + json.dumps(specifier) + ";" + worker("42"), error=["failed to resolve"] if name == "alias-prefix-denied" else ["unknown"], aliases=alias)
    for name, filename, fragments in [("alias-missing-target", "absent.js", ["fixture-plugin", "could not be read"]), ("alias-missing-sibling", "missing-user.js", ["fixture-plugin", "absent.js"]), ("alias-typescript-rejected", "typed.ts", ["pre-compiled ESM JavaScript"])]:
        add(name, "import 'fixture-alias';" + worker("42"), error=fragments, aliases=[{"specifier": "fixture-alias", "target": str(root / "canonical" / filename), "plugin": "fixture-plugin"}])
    add("prepared-virtual-evaluation-once", "import * as a from 'virtual:fixture'; const b=await import('virtual:fixture');" + worker("[a===b,a.marker,virtualCount]"), [True, 42, 1], virtual_modules=[{"specifier": "virtual:fixture", "source": "globalThis.virtualCount=(globalThis.virtualCount||0)+1; export const marker=42;", "plugin": "fixture-plugin"}])
    add("node-stubs-present-use-fails", "import fs from 'node:fs';" + worker("(()=>{try{fs.readFileSync('/nothing');return null}catch(e){return e.message}})()"), "node:* is not available under the SSG runtime")
    add("authored-ext-import-rejected", "import 'ext:zfb_node_stubs/node_stub.js';" + worker("42"), error=["Importing ext: modules is only allowed"])
    add("unknown-node-rejected", "import 'node:os';" + worker("42"), error=["unknown"])
    add("wasm-compiled-uninstantiated-consumed", "import module from './answer%20space.wasm';" + worker("[module instanceof WebAssembly.Module,new WebAssembly.Instance(module).exports.answer()]"), [True, 42], asset_root=str(root / "bundle"))
    for name, specifier, fragments in [("wasm-missing", "./missing.wasm", ["missing.wasm"]), ("wasm-malformed", "./broken.wasm", ["WebAssembly"]), ("wasm-encoded-traversal", "./%2e%2e%2foutside.wasm", ["escapes"]), ("wasm-symlink-escape", "./escape.wasm", ["escapes"]), ("wasm-general-file-denied", (root / "bundle/answer space.wasm").as_uri(), ["unknown"])]:
        add(name, "import module from " + json.dumps(specifier) + ";" + worker("42"), error=fragments, asset_root=str(root / "bundle"))
    add("wasm-memory-source-precedence", "import value from './broken.wasm';" + worker("value"), 42, asset_root=str(root / "bundle"), modules=[{"specifier": "file:///zfb/broken.wasm", "source": "export default 42;"}])
    add("fresh-host-after-failures", worker("typeof globalThis.moduleCount"), "undefined")
    add("syntax-error-origin", "import './syntax-dependency.mjs';" + worker("42"), error=["SyntaxError", "file:///zfb/syntax-dependency.mjs:3:14"], modules=[{"specifier": "file:///zfb/syntax-dependency.mjs", "source": "\n\nexport const = ;"}])
    def utility(name, source, exports=(), calls=(), value=None, error=None):
        result.append({"name": name, "request": {"kind": "utility", "name": "file:///zfb/utility.mjs", "source": source, "exports": exports, "calls": calls}, "expected": {"ok": error is None, "value": value, "error": error}})
    utility("utility-json-and-async-default", "export const data={answer:42,nested:[true,null,'ok']};export default async p=>'hello '+p.name;", ["data"], [{"name": "Ada"}, {"name": "Lin"}], {"exports": {"data": {"answer": 42, "nested": [True, None, "ok"]}}, "calls": ["hello Ada", "hello Lin"], "console": ""})
    utility("utility-sync-string-coercion", "export default ()=>42;", calls=[{}], value={"exports": {}, "calls": ["42"], "console": ""})
    utility("utility-without-worker-shape", "export const data=42;", ["data"], value={"exports": {"data": 42}, "calls": [], "console": ""})
    utility("utility-noncallable-default", "export default {fetch(){}};", calls=[{}], error=["callable"])
    utility("utility-boxed-string-and-map", "export const boxed=new String('hi');export const map=new Map([['answer',42],['nested',{ok:true}]]);", ["boxed", "map"], value={"exports": {"boxed": {"0": "h", "1": "i"}, "map": {"answer": 42, "nested": {"ok": True}}}, "calls": [], "console": ""})
    utility("utility-map-nonstring-key-rejected", "export const map=new Map([[42,'answer']]);", ["map"], error=["string"])
    return result


def run(binary, engine, rows):
    wire = "".join(json.dumps(row["request"]) + "\n" for row in rows)
    try:
        proc = subprocess.run([str(binary.resolve())], input=wire, text=True,
                              capture_output=True, timeout=60, check=False)
    except subprocess.TimeoutExpired as error:
        return {"engine": engine, "passed": False, "watchdog": str(error)}
    receipts = [json.loads(line[len("ZFB_HOST_RECEIPT "):]) for line in proc.stdout.splitlines() if line.startswith("ZFB_HOST_RECEIPT ")]
    results = []
    for row, receipt in zip(rows, receipts):
        expected = row["expected"]
        errors = []
        if receipt.get("engine") != engine or receipt.get("ok") != expected["ok"]:
            errors.append("engine or success status differs")
        elif expected["ok"] and "value" in expected:
            if json.dumps(receipt.get("value"), sort_keys=True) != json.dumps(expected["value"], sort_keys=True):
                errors.append("literal utility value differs")
        elif expected["ok"]:
            responses = receipt["value"]["responses"]
            if "raw" in expected:
                if responses != [expected["raw"]]:
                    errors.append("binary response or ordered headers differ")
            else:
                values = [json.loads(bytes(response["body"]).decode()) for response in responses]
                want = expected.get("jsonResponses", [expected["json"]])
                if json.dumps(values, sort_keys=True) != json.dumps(want, sort_keys=True):
                    errors.append("literal JSON differs")
                if any(response["status"] != 200 for response in responses):
                    errors.append("status differs")
        else:
            for fragment in expected["error"]:
                if fragment not in receipt.get("error", ""):
                    errors.append("missing error fragment: " + fragment)
        if expected.get("consoleContains", "") not in receipt.get("console", ""):
            errors.append("console diagnostic missing")
        results.append({"name": row["name"], "passed": not errors, "failures": errors, "receipt": receipt})
    return {"engine": engine, "executable_sha256": digest(binary), "exit_code": proc.returncode,
            "expected_count": len(rows), "receipt_count": len(receipts), "stderr": proc.stderr,
            "passed": proc.returncode == 0 and len(rows) == len(receipts) and all(r["passed"] for r in results), "rows": results}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--fixture-dir", type=Path, required=True, help="New ignored directory, frozen before either runtime starts")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.fixture_dir.resolve()
    manifest = fixtures(root)
    rows = cases(root)
    (root / "requests.json").write_text(json.dumps(rows, indent=2) + "\n")
    results = [run(args.baseline, "deno_core 0.399.0 / v8 147.4.0", rows)]
    if args.candidate:
        results.append(run(args.candidate, "direct v8 147.4.0", rows))
    args.output.write_text(json.dumps({"fixture_manifest": manifest, "requests_sha256": digest(root / "requests.json"), "results": results}, indent=2) + "\n")
    for result in results:
        print(result["engine"], "PASS" if result["passed"] else "FAIL", sum(r["passed"] for r in result.get("rows", [])), "/", len(rows))
        for row in result.get("rows", []):
            if not row["passed"]:
                print(row["name"], row["failures"], row["receipt"])
    return 0 if all(r["passed"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
