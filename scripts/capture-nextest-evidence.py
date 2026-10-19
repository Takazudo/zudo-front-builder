#!/usr/bin/env python3
"""Capture one nextest 0.9.140 invocation; see capture-nextest-evidence.md."""

import argparse
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib
import xml.etree.ElementTree as ET


class EvidenceError(Exception):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest()


def checked_json(command, env=None):
    result = subprocess.run(command, capture_output=True, text=True, env=env)
    if result.returncode:
        raise EvidenceError(f"preflight failed: {command!r}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def merge(left, right):
    for key, value in right.items():
        if isinstance(value, dict) and isinstance(left.get(key), dict):
            merge(left[key], value)
        else:
            left[key] = value
    return left


def source_fingerprint(workspace):
    names = subprocess.check_output(["git", "-C", str(workspace), "ls-files", "--cached", "--others", "--exclude-standard", "-z"])
    result = hashlib.sha256()
    for name in sorted(set(names.split(b"\0")) - {b""}):
        path = workspace / os.fsdecode(name)
        result.update(name + b"\0")
        if path.is_symlink():
            value = b"symlink:" + os.fsencode(os.readlink(path))
        elif path.is_file():
            value = b"file:" + path.read_bytes()
        else:
            value = b"missing-or-non-file"
        result.update(hashlib.sha256(value).digest())
    return result.hexdigest()


def resolve(args):
    if "MIRI_SYSROOT" in os.environ:
        raise EvidenceError("Miri profile selection is not supported by this capture helper")
    version = subprocess.check_output([args.cargo, "nextest", "--version"], text=True).strip()
    # Fail closed if upstream changes store/config semantics; no guessed path fallback.
    if not re.match(r"^cargo-nextest 0\.9\.140(?:\s|$)", version):
        raise EvidenceError(f"unsupported nextest version (audited: 0.9.140): {version}")
    metadata_args = [args.cargo, "metadata", "--format-version", "1", "--no-deps", "--offline"]
    if args.manifest_path:
        metadata_args += ["--manifest-path", args.manifest_path]
    env = os.environ.copy()
    if args.target_dir:
        env["CARGO_TARGET_DIR"] = args.target_dir
    metadata = checked_json(metadata_args, env)
    workspace = Path(metadata["workspace_root"])
    if not workspace.is_absolute() or not Path(metadata["target_directory"]).is_absolute():
        raise EvidenceError("Cargo metadata paths must be absolute")
    source = subprocess.check_output(["git", "-C", str(workspace), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(["git", "-C", str(workspace), "status", "--porcelain=v1", "-uall"], text=True)
    config = {"store": {"dir": "target/nextest"}, "profile": {"default": {}}}
    paths = []
    for spec in reversed(args.tool_config_file):
        tool, separator, path = spec.partition(":")
        if not tool or not separator or not Path(path).is_absolute():
            raise EvidenceError("tool config must be TOOL:ABSOLUTE_PATH")
        paths.append((Path(path), True))
    paths.append((Path(args.config_file) if args.config_file else workspace / ".config/nextest.toml", bool(args.config_file)))
    configs = []
    for path, required in paths:
        path = path.absolute()
        if not required and not path.exists():
            configs.append({"path": str(path), "sha256": None})
            continue
        raw = path.read_bytes()
        merge(config, tomllib.loads(raw.decode("utf-8")))
        configs.append({"path": str(path), "sha256": digest(raw)})
    profile = args.profile or os.environ.get("NEXTEST_PROFILE") or "default"
    profiles = config["profile"]
    current, seen, junit_path = profile, set(), None
    while True:
        if current in seen or current not in profiles:
            raise EvidenceError(f"invalid profile inheritance: {current}")
        seen.add(current)
        settings = profiles[current]
        if junit_path is None:
            junit_path = settings.get("junit", {}).get("path")
        if current == "default":
            break
        current = settings.get("inherits", "default")
    if not isinstance(junit_path, str) or not junit_path:
        raise EvidenceError(f"JUnit reporting is not enabled for profile {profile}")
    store = config["store"]["dir"]
    if not isinstance(store, str) or not store:
        raise EvidenceError("invalid nextest store.dir")
    # Upstream 0.9.140: workspace_root.join(store.dir).join(profile).join(junit.path).
    report = Path(os.path.abspath(workspace / store / profile / junit_path))
    return report, {"source_sha": source, "source_dirty": bool(dirty), "source_status": dirty, "source_fingerprint": source_fingerprint(workspace),
                    "workspace_root": str(workspace), "cargo_target_directory": metadata["target_directory"],
                    "profile": profile, "nextest_version": version, "config_inputs": configs,
                    "junit_destination": str(report)}


def capture_report(report, output, marker_ns, started, previous):
    if report.is_symlink() or not report.is_file():
        raise EvidenceError("missing original JUnit regular file")
    before = report.stat()
    raw = report.read_bytes()
    after = report.stat()
    if (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
        raise EvidenceError("JUnit changed during capture")
    # Preserve even rejected bytes for diagnosis, without calling them accepted evidence.
    (output / "candidate.xml").write_bytes(raw)
    if before.st_mtime_ns < marker_ns:
        raise EvidenceError("stale JUnit file timestamp")
    root = ET.fromstring(raw)
    if root.tag != "testsuites" or not root.get("uuid") or not root.get("timestamp"):
        raise EvidenceError("not an original nextest JUnit report (root/uuid/timestamp missing)")
    timestamp = dt.datetime.fromisoformat(root.attrib["timestamp"].replace("Z", "+00:00"))
    if timestamp.tzinfo is None or timestamp.timestamp() < int(started * 1000) / 1000 or timestamp.timestamp() > time.time():
        raise EvidenceError("stale or invalid JUnit run timestamp")
    if previous and digest(raw) == previous:
        raise EvidenceError("stale JUnit bytes from preceding command")
    return {"status": "captured", "sha256": digest(raw), "bytes": len(raw), "run_uuid": root.attrib["uuid"]}


def run(args, forwarded):
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    command = [args.cargo, "nextest", "run"]
    for option in ("profile", "config_file", "manifest_path", "target_dir"):
        value = getattr(args, option)
        if value is not None:
            command += ["--" + option.replace("_", "-"), value]
    for value in args.tool_config_file:
        command += ["--tool-config-file", value]
    command += forwarded
    receipt = {"schema": 1, "command": command, "cwd": str(Path.cwd()), "child_exit": None,
               "execution": "not_started", "evidence": {"status": "missing"}}
    returned = 2
    try:
        forbidden = {"--profile", "-P", "--config-file", "--tool-config-file", "--manifest-path", "--target-dir",
                     "--config", "--cargo-metadata", "--binaries-metadata", "--archive-file", "--workspace-remap",
                     "--target-dir-remap", "--extract-to", "--no-run", "--help", "-h", "--version", "-V"}
        for word in forwarded:
            key = word.split("=", 1)[0]
            if key in forbidden or word.startswith("-P"):
                raise EvidenceError(f"unsupported forwarded option {word}; pass resolution options before --; compile separately")
        report, identity = resolve(args)
        receipt.update(identity)
        workspace = Path(identity["workspace_root"]).resolve()
        if output == workspace or workspace in output.parents:
            raise EvidenceError("evidence directory must be outside the source workspace")
        if output == report.resolve() or output in report.resolve().parents:
            raise EvidenceError("evidence directory must be separate from nextest report destination")
        report.parent.mkdir(parents=True, exist_ok=True)
        # Cooperating wrappers serialize by physical report destination, including absolute junit paths.
        lock_path = report.parent / (".capture-" + digest(str(report.resolve()).encode()) + ".lock")
        with lock_path.open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise EvidenceError("another capture owns this JUnit destination") from error
            previous = None
            if report.is_symlink():
                raise EvidenceError("refusing symlink JUnit destination")
            if report.exists():
                raw = report.read_bytes()
                (output / "preexisting.xml").write_bytes(raw)
                previous = digest(raw)
                report.unlink()
            marker = report.parent / (lock_path.name + ".started")
            marker.write_bytes(b"")
            marker_ns = marker.stat().st_mtime_ns
            started = time.time()
            receipt["started_utc"] = dt.datetime.fromtimestamp(started, dt.timezone.utc).isoformat()
            try:
                with (output / "stdout.log").open("wb") as stdout, (output / "stderr.log").open("wb") as stderr:
                    child = subprocess.run(command, stdout=stdout, stderr=stderr)
                receipt["child_exit"] = child.returncode
                receipt["execution"] = "command_completed"
                returned = child.returncode if child.returncode >= 0 else 128 - child.returncode
                receipt["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
                try:
                    receipt["evidence"] = capture_report(report, output, marker_ns, started, previous)
                    ending_sha = subprocess.check_output(["git", "-C", identity["workspace_root"], "rev-parse", "HEAD"], text=True).strip()
                    receipt["source_sha_after"] = ending_sha
                    receipt["source_fingerprint_after"] = source_fingerprint(workspace)
                    if receipt["source_fingerprint_after"] != identity["source_fingerprint"]:
                        raise EvidenceError("source files changed during command")
                    if ending_sha != identity["source_sha"]:
                        raise EvidenceError("source HEAD changed during command")
                    for item in identity["config_inputs"]:
                        path = Path(item["path"])
                        current = digest(path.read_bytes()) if path.exists() else None
                        if current != item["sha256"]:
                            raise EvidenceError("nextest configuration changed during command")
                    (output / "candidate.xml").rename(output / "junit.xml")
                except (EvidenceError, OSError, ValueError, ET.ParseError) as error:
                    receipt["evidence"] = {"status": "missing", "reason": str(error)}
                    if returned == 0:
                        returned = 4
            finally:
                marker.unlink(missing_ok=True)
    except (EvidenceError, OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        receipt["evidence"] = {"status": "missing", "reason": str(error)}
        if receipt["child_exit"] is not None:
            code = receipt["child_exit"]
            returned = (code if code >= 0 else 128 - code) or 4
    finally:
        receipt["wrapper_exit"] = returned
        try:
            (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        except OSError as error:
            print(f"cannot write receipt: {error}", file=sys.stderr)
            return returned if receipt["child_exit"] else 4
    print(f"evidence={receipt['evidence']['status']} child_exit={receipt['child_exit']} receipt={output / 'receipt.json'}", file=sys.stderr)
    return returned


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, help="new command-specific evidence directory")
    parser.add_argument("--cargo", default="cargo")
    for name in ("profile", "config-file", "manifest-path", "target-dir"):
        parser.add_argument("--" + name)
    parser.add_argument("--tool-config-file", action="append", default=[])
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not args.arguments or args.arguments[0] != "--":
        parser.error("supply nextest run arguments after --")
    try:
        return run(args, args.arguments[1:])
    except OSError as error:
        print(f"cannot create/write evidence directory: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
