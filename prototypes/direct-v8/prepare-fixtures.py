#!/usr/bin/env python3
"""Prepare #4240 inputs through the current CLI and its native compiler, then freeze.

Run only after #4239 passes, in the manager's heavy lane. This script never
builds Rust, selects a candidate executor, downloads tools, or uploads assets.
A prebuilt zfb lib-test binary invokes the actual private dev-wrapper producer.
Since #4635 zfb embeds its bundler and needs no esbuild: the standalone config,
negative and plugin bundles go through the same `zfb __zfb-compiler-worker`
producer the CLI's config loader uses, pinned to the checkout's BACKEND_IDENTITY.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).resolve().parent
WRAPPER_TEST = "commands::dev::tests::direct_v8_export_canonical_wrapper"
# Private zfb argv and wire version from crates/zfb-compiler-worker
# (launcher.rs PRIVATE_ARG, protocol.rs PROTOCOL_VERSION).
WORKER_ARG = "__zfb-compiler-worker"
WORKER_PROTOCOL = 1
BACKEND_SOURCE = "crates/zfb-bundler/src/request.rs"


def expected_bundler(root=ROOT):
    match = re.search(r'pub const BACKEND_IDENTITY: &str = "([^"]+)";',
                      (root / BACKEND_SOURCE).read_text())
    if not match:
        raise RuntimeError(f"no BACKEND_IDENTITY in {BACKEND_SOURCE}")
    return match.group(1)


def embedded_bundler(version_report):
    found = re.findall(r"^embedded bundler: (\S+)$", version_report, re.M)
    if len(found) != 1:
        raise RuntimeError("zfb --version must report exactly one 'embedded bundler:' line")
    return found[0]


def bundle_request(cwd, entry, outfile, *, target, aliases=(), externals=(), source_map="None"):
    """A zfb_bundler::BundleRequest in its serde JSON shape: ESM, neutral platform, one file."""
    return {"cwd": str(cwd), "entry": str(entry),
            "output": {"File": {"file": str(outfile), "asset_names": None}},
            "format": "Esm", "platform": "Neutral", "target": target,
            "jsx": {"runtime": None, "import_source": None}, "tsconfig": None,
            "resolve": {"aliases": [[key, value] for key, value in aliases], "conditions": None,
                        "main_fields": None, "preserve_symlinks": False, "extra_module_dirs": [],
                        "packages": "Bundle"},
            "defines": [], "loaders": [], "externals": [{"Package": name} for name in externals],
            "splitting": False, "minify": False, "keep_names": False, "source_map": source_map,
            "banner": None, "graph": None}


def publish_worker_files(response, job_id, nonce, private_dir, expected):
    """Verify one compiler-worker response and copy exactly the expected files out.

    `expected` is the exact set of absolute destinations the request owns.
    Returns the engine warnings, which the manifest records.
    """
    if (response.get("protocol"), response.get("job_id"), response.get("nonce")) != (
            WORKER_PROTOCOL, job_id, nonce):
        raise RuntimeError("compiler worker answered with a different protocol or job identity")
    outcome = response.get("outcome") or {}
    if set(outcome) != {"Generated"}:
        raise RuntimeError(f"native bundle did not generate: {json.dumps(outcome)}")
    generated = outcome["Generated"]
    files_dir = (private_dir / "files").resolve()
    reported = {}
    for entry in generated["files"]:
        source = (private_dir / entry["private"]).resolve()
        destination = Path(entry["destination"])
        if not source.is_relative_to(files_dir) or not source.is_file():
            raise RuntimeError(f"compiler worker reported a file outside its private directory: {entry}")
        if source.stat().st_size != entry["size"] or sha(source) != entry["sha256"]:
            raise RuntimeError(f"compiler worker misreported {destination}")
        if destination in reported:
            raise RuntimeError(f"compiler worker reported {destination} twice")
        reported[destination] = source
    if set(reported) != set(expected):
        raise RuntimeError(f"native bundle emitted {sorted(map(str, reported))}, "
                           f"expected {sorted(map(str, expected))}")
    if {path.resolve() for path in files_dir.iterdir()} != set(reported.values()):
        raise RuntimeError("compiler worker left an unreported private file")
    for destination, source in reported.items():
        if destination.exists():
            raise RuntimeError(f"native bundle would overwrite {destination}")
        shutil.copyfile(source, destination)
    return generated["warnings"]


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
    # Preserve executable spelling: resolving a rustc symlink to rustup changes
    # argv[0] and reports the wrong tool. File hashing still follows the link.
    tools = {name: getattr(args, name).absolute() for name in ("zfb", "wrapper_test_binary", "rustc", "node")}
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
    # The zfb under test bundles natively; a stale esbuild override from the
    # caller's shell must not reach any producer command.
    environment.pop("ZFB_ESBUILD_BIN", None)
    environment["ZFB_NODE_BIN"] = str(tools["node"])
    worker_private = out / "worker-private"
    worker_private.mkdir()
    bundle_warnings = {}

    def run(label, argv, cwd=ROOT, extra_env=None, timeout=180, stdin=None):
        env = environment | (extra_env or {})
        try:
            result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env, capture_output=True,
                                    text=True, timeout=timeout, check=False, input=stdin)
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

    def native_bundle(label, cwd, request, expected):
        """Bundle through `zfb __zfb-compiler-worker`, the CLI's own native producer."""
        private_dir = worker_private / label
        private_dir.mkdir()
        job_id, nonce = label, secrets.token_hex(16)
        (logs / f"{label}.request.json").write_text(json.dumps(request, indent=2) + "\n")
        stdout = run(label, [tools["zfb"], WORKER_ARG], cwd, stdin=json.dumps(
            {"protocol": WORKER_PROTOCOL, "job_id": job_id, "nonce": nonce,
             "private_dir": str(private_dir), "request": request, "test_hook": None}))
        bundle_warnings[label] = publish_worker_files(json.loads(stdout), job_id, nonce, private_dir,
                                                      [cwd / name for name in expected])

    try:
        versions = {"zfb": run("zfb-version", [tools["zfb"], "-V"]),
                    "zfb_long": run("zfb-long-version", [tools["zfb"], "--version"]),
                    "rustc": run("rustc-version", [tools["rustc"], "-vV"]),
                    "node": run("node-version", [tools["node"], "--version"])}
        bundler = embedded_bundler(versions["zfb_long"])
        pin = expected_bundler()
        if bundler != pin:
            raise RuntimeError(f"zfb embeds bundler {bundler}, but {BACKEND_SOURCE} pins {pin}")
        versions["bundler"] = bundler
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
        # The config loader's own request (crates/zfb-config-loader/src/loader.rs
        # bundle_config); source-map evidence is the separate negative below,
        # not an invented config-loader requirement.
        native_bundle("config-bundle", config, bundle_request(
            config, config / "zfb.config.ts", config / "config.mjs", target="esnext",
            aliases=[("zfb/config", str(stub)), ("@takazudo/zfb/config", str(stub))]), ["config.mjs"])
        negative = frozen / "negative"
        shutil.copytree(FIXTURES / "negative", negative)
        native_bundle("negative-bundle", negative, bundle_request(
            negative, negative / "throw.ts", negative / "throw.mjs", target="es2022",
            source_map="Linked"), ["throw.mjs", "throw.mjs.map"])
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
  if (new URL(request.url).pathname === "/binary") {
    return new Response(new Uint8Array([0,255,128,65]), {status:201,headers:[["content-type","application/octet-stream"],["set-cookie","binary=one"],["set-cookie","binary=two"],["x-prepared","left"],["x-prepared","right"]]});
  }
  return Response.json({answer,message,method:request.method,marker:request.headers.get("x-input"),body:Array.from(new Uint8Array(await request.arrayBuffer()))}, {headers:[["set-cookie","plugin=one"],["set-cookie","plugin=two"],["x-prepared","42"]]});
}};
''')
        native_bundle("plugin-bundle", plugin, bundle_request(
            plugin, plugin / "entry.ts", plugin / "worker.mjs", target="es2022",
            externals=["plugin:answer", "virtual:message"], source_map="Linked"),
            ["worker.mjs", "worker.mjs.map"])
        after = source_state()
        if before != after:
            raise RuntimeError("repository inputs changed during production; keep logs and regenerate from a stable tree")
        if tool_hashes != {name: sha(path) for name, path in tools.items()}:
            raise RuntimeError("producer executable bytes changed during production")
        manifest = {"schema": 1, "producer": before, "producer_build_command_supplied": args.producer_build_command,
                    "tools": {name: {"path": str(path), "sha256": sha(path)} for name, path in tools.items()},
                    "versions": versions, "environment": {name: environment[name] for name in (
                        "ZFB_NODE_BIN", "CARGO_BUILD_JOBS",
                        "CARGO_INCREMENTAL", "CARGO_TARGET_DIR", "RUSTUP_TOOLCHAIN") if name in environment},
                    "commands": commands, "files": inventory(frozen),
                    "policy": {"normal_cli_host": "deno_core 0.399.0 / v8 147.4.0",
                               "bundler": bundler, "bundle_warnings": bundle_warnings,
                               "node_role": "producer/orchestrator only",
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
