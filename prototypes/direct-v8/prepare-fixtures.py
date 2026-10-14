#!/usr/bin/env python3
"""Prepare #4240 inputs through the current CLI and pinned esbuild, then freeze.

Run only after #4239 passes, in the manager's heavy lane. This script never
builds Rust, selects a candidate executor, downloads tools, or uploads assets.
A prebuilt zfb lib-test binary invokes the actual private dev-wrapper producer.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).resolve().parent
WRAPPER_TEST = "commands::dev::tests::direct_v8_export_canonical_wrapper"


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def source_state():
    changed = set(git("diff", "--name-only", "-z", "HEAD").decode().split("\0"))
    changed.update(git("ls-files", "--others", "--exclude-standard", "-z").decode().split("\0"))
    return {"commit": git("rev-parse", "HEAD").decode().strip(),
            "status": git("status", "--porcelain=v1").decode(),
            "tracked_diff_sha256": hashlib.sha256(git("diff", "--binary", "HEAD")).hexdigest(),
            "changed_files": {name: sha(ROOT / name) if (ROOT / name).is_file() else None
                              for name in sorted(changed) if name},
            "policy_inputs": {name: sha(ROOT / name) for name in (
                "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "pnpm-lock.yaml",
                "package.json", "packages/zfb/package.json", "crates/zfb-toolchain-pins/src/lib.rs")}}


def inventory(root):
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"frozen snapshot cannot contain symlinks: {path}")
        if path.is_file() and path != root / "manifest.json":
            result[path.relative_to(root).as_posix()] = {"bytes": path.stat().st_size, "sha256": sha(path)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zfb", type=Path, required=True)
    parser.add_argument("--esbuild", type=Path, required=True)
    parser.add_argument("--wrapper-test-binary", type=Path, required=True)
    parser.add_argument("--rustc", type=Path, required=True)
    parser.add_argument("--node", type=Path, required=True)
    parser.add_argument("--producer-build-command", required=True,
                        help="Actual manager build command; recorded as supplied provenance")
    parser.add_argument("--output", type=Path, required=True, help="New directory below repository target/")
    args = parser.parse_args()
    out = args.output.resolve()
    if not out.is_relative_to(ROOT / "target") or out == ROOT / "target" or out.exists():
        parser.error("--output must be a new directory below this repository's ignored target/")
    tools = {name: getattr(args, name).resolve() for name in ("zfb", "esbuild", "wrapper_test_binary", "rustc", "node")}
    for name, path in tools.items():
        if not path.is_file():
            parser.error(f"missing explicit {name} executable: {path}")
    before = source_state()
    tool_hashes = {name: sha(path) for name, path in tools.items()}
    out.mkdir(parents=True)
    frozen = out / "frozen"
    frozen.mkdir()
    logs = frozen / "producer-logs"
    logs.mkdir()
    commands = []
    environment = os.environ.copy()
    # The same explicit pin drives CLI preparation and standalone config/error
    # emission. Never use whichever esbuild happens to appear on PATH.
    environment["ZFB_ESBUILD_BIN"] = str(tools["esbuild"])
    environment["ZFB_NODE_BIN"] = str(tools["node"])

    def run(label, argv, cwd=ROOT, extra_env=None, timeout=180):
        env = environment | (extra_env or {})
        try:
            result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env, capture_output=True,
                                    text=True, timeout=timeout, check=False)
        except subprocess.TimeoutExpired as error:
            for channel in ("stdout", "stderr"):
                partial = getattr(error, channel) or b""
                (logs / f"{label}.{channel}").write_bytes(partial if isinstance(partial, bytes) else partial.encode())
            commands.append({"label": label, "argv": [str(x) for x in argv], "cwd": str(cwd),
                             "watchdog_seconds": timeout, "passed": False, "extra_env": extra_env or {}})
            raise RuntimeError(f"{label} watchdog expired; this is failed production, not clean shutdown evidence") from error
        (logs / f"{label}.stdout").write_text(result.stdout)
        (logs / f"{label}.stderr").write_text(result.stderr)
        commands.append({"label": label, "argv": [str(x) for x in argv], "cwd": str(cwd),
                         "exit_code": result.returncode, "extra_env": extra_env or {}})
        if result.returncode:
            raise RuntimeError(f"{label} failed ({result.returncode}); see {logs}")
        return result.stdout.strip()

    try:
        versions = {"zfb": run("zfb-version", [tools["zfb"], "-V"]),
                    "esbuild": run("esbuild-version", [tools["esbuild"], "--version"]),
                    "rustc": run("rustc-version", [tools["rustc"], "-vV"]),
                    "node": run("node-version", [tools["node"], "--version"])}
        pin = re.search(r'EXPECTED_ESBUILD_VERSION: &str = "([^"]+)"',
                        (ROOT / "crates/zfb-toolchain-pins/src/lib.rs").read_text()).group(1)
        if versions["esbuild"] != pin:
            raise RuntimeError(f"esbuild {versions['esbuild']} differs from repository pin {pin}")
        site = out / "site"
        shutil.copytree(FIXTURES / "site", site)
        run("real-zfb-build", [tools["zfb"], "build"], site)
        bundle = site / ".zfb-build/bundle.mjs"
        linked_map = site / ".zfb-build/bundle.mjs.map"
        if not bundle.is_file() or not linked_map.is_file():
            raise RuntimeError("real CLI did not emit bundle.mjs and its linked source map")
        run("canonical-wrapper", [tools["wrapper_test_binary"], WRAPPER_TEST,
            "--exact", "--ignored", "--nocapture"], extra_env={
                "ZFB_DIRECT_V8_SITE": str(site),
                "ZFB_DIRECT_V8_WRAPPER_OUT": str(frozen / "wrapper")})
        wrapper_receipt = json.loads((frozen / "wrapper/receipt.json").read_text())
        if wrapper_receipt["bundle_sha256"] != sha(bundle) or wrapper_receipt["route_count"] == 0:
            raise RuntimeError("canonical wrapper receipt does not bind the real inner bundle")
        main_dir = frozen / "main"
        main_dir.mkdir()
        for path in [bundle, linked_map, *sorted(bundle.parent.rglob("*.wasm"))]:
            relative = path.relative_to(bundle.parent)
            destination = main_dir / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)
        if not list(main_dir.rglob("*.wasm")):
            raise RuntimeError("real CLI emitted no copied Wasm asset")
        shutil.copytree(site / "dist", frozen / "dist")
        shutil.copytree(FIXTURES / "site", frozen / "authored-site")
        shutil.copy2(FIXTURES / "site/expected.json", frozen / "expected.json")

        config = frozen / "config"
        config.mkdir()
        shutil.copy2(site / "zfb.config.ts", config / "zfb.config.ts")
        stub = config / "zfb-config-stub.mjs"
        shutil.copy2(ROOT / "crates/zfb-config-loader/js/zfb-config-stub.mjs", stub)
        # Exact current loader preparation flags; source-map evidence is the
        # separate negative below, not an invented config-loader requirement.
        run("config-bundle", [tools["esbuild"], "--bundle", "--format=esm", "--platform=neutral",
            "--target=esnext", "--log-level=warning", f"--alias:zfb/config={stub}",
            f"--alias:@takazudo/zfb/config={stub}", "--outfile=config.mjs", "zfb.config.ts"], config)
        negative = frozen / "negative"
        shutil.copytree(FIXTURES / "negative", negative)
        run("negative-bundle", [tools["esbuild"], "throw.ts", "--bundle", "--format=esm",
            "--platform=neutral", "--target=es2022", "--sourcemap=linked", "--outfile=throw.mjs"], negative)
        # This isolated module-policy probe is deliberately distinct from the
        # real generated site; it does not stand in for ZFB pipeline coverage.
        plugin = frozen / "plugin"
        plugin.mkdir()
        (plugin / "alias.mjs").write_text('export const answer = 42;\n')
        (plugin / "virtual.mjs").write_text('export const message = "virtual-ok";\n')
        (plugin / "entry.ts").write_text('''import { answer } from "plugin:answer";
import { message } from "virtual:message";
export default { async fetch(request: Request) {
  console.log("prepared plugin request");
  return Response.json({answer,message,method:request.method,marker:request.headers.get("x-input"),body:Array.from(new Uint8Array(await request.arrayBuffer()))}, {headers:[["set-cookie","plugin=one"],["set-cookie","plugin=two"],["x-prepared","42"]]});
}};
''')
        run("plugin-bundle", [tools["esbuild"], "entry.ts", "--bundle", "--format=esm",
            "--platform=neutral", "--target=es2022", "--external:plugin:answer",
            "--external:virtual:message", "--sourcemap=linked", "--outfile=worker.mjs"], plugin)
        after = source_state()
        if before != after:
            raise RuntimeError("repository inputs changed during production; keep logs and regenerate from a stable tree")
        if tool_hashes != {name: sha(path) for name, path in tools.items()}:
            raise RuntimeError("producer executable bytes changed during production")
        manifest = {"schema": 1, "producer": before, "producer_build_command_supplied": args.producer_build_command,
                    "tools": {name: {"path": str(path), "sha256": sha(path)} for name, path in tools.items()},
                    "versions": versions, "environment": {name: environment[name] for name in (
                        "ZFB_ESBUILD_BIN", "ZFB_NODE_BIN", "ZFB_V8_ICU_DATA", "CARGO_BUILD_JOBS",
                        "CARGO_INCREMENTAL", "CARGO_TARGET_DIR", "RUSTUP_TOOLCHAIN") if name in environment},
                    "commands": commands, "files": inventory(frozen),
                    "policy": {"normal_cli_host": "deno_core 0.399.0 / v8 147.4.0",
                               "esbuild": pin, "node_role": "producer/orchestrator only",
                               "wrapper": wrapper_receipt, "config_expected": {
                                   "outDir": "dist", "collections": [{"name": "posts", "path": "content/posts"}]},
                               "negative_expected": {"file_suffix": "throw.ts", "line": 6, "column": 9,
                                   "message": "direct-v8-authored-map-sentinel", "console": "direct-v8-negative-before-throw"}}}
        (frozen / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"Frozen {len(manifest['files'])} files: {frozen}")
        return 0
    except Exception as error:
        (out / "producer-failure.json").write_text(json.dumps({"error": str(error), "commands": commands,
                                                             "producer": before}, indent=2) + "\n")
        raise


if __name__ == "__main__":
    sys.exit(main())
