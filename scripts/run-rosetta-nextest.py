#!/usr/bin/env python3
"""Build once, warm the exact worker host, then reuse nextest 0.9.140 metadata.

Local qualification only. Run under the heavy guard with an x64 toolchain PATH.
Metadata schema: nextest-metadata/src/test_list.rs at cargo-nextest-0.9.140.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time
import xml.etree.ElementTree as ET


class Refusal(Exception):
    pass


def require(ok, message):
    if not ok:
        raise Refusal(message)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def output(argv):
    return subprocess.check_output(argv, text=True).strip()


def identity(path):
    require(path.is_file() and not path.is_symlink(), f"not a regular binary: {path}")
    st = path.stat()
    require(os.access(path, os.X_OK), f"not executable: {path}")
    return dict(device=st.st_dev, inode=st.st_ino, size=st.st_size, sha256=sha(path))


def x64(path):
    desc = output(["/usr/bin/file", "-b", str(path)])
    require("Mach-O 64-bit executable x86_64" in desc and "arm64" not in desc,
            f"expected thin x86_64 Mach-O: {path}: {desc}")
    return desc


def host_from_metadata(data, cargo):
    # Names and relative-path semantics are pinned to nextest 0.9.140.
    require(set(data) == {"rust-build-meta", "rust-binaries"}, "unsupported binary metadata envelope")
    meta = data["rust-build-meta"]
    target = Path(meta["target-directory"])
    require(target.is_absolute() and target.resolve() == Path(cargo["target_directory"]).resolve(),
            "target-directory mismatch")
    packages = [p["id"] for p in cargo["packages"] if p["name"] == "zfb-compiler-worker"]
    require(len(packages) == 1, "worker package missing or ambiguous")
    matches = [v for v in meta["non-test-binaries"].get(packages[0], [])
               if v["name"] == "compiler-worker-test-host" and v["kind"] == "bin"]
    require(len(matches) == 1, "worker host missing or ambiguous in metadata")
    relative = Path(matches[0]["path"])
    require(not relative.is_absolute() and ".." not in relative.parts, "unsafe host path")
    host = target / relative
    require(host.resolve().is_relative_to(target.resolve()), "host escapes target")
    require(any(v.get("binary-name") == "lifecycle" and v.get("package-id") == packages[0]
                for v in data["rust-binaries"].values()), "lifecycle binary absent")
    return host


def warm(host, before, timeout=30):
    require(identity(host) == before, "host changed before warm")
    started = time.time_ns()
    result = subprocess.run([str(host)], stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)
    require(result.returncode == 64 and result.stdout == b"" and result.stderr ==
            b"compiler-worker-test-host: started without the private worker argument\n",
            f"unexpected warm result: exit {result.returncode}, {result.stderr!r}")
    require(identity(host) == before, "host changed during warm")
    return {"exit": result.returncode, "stdout": result.stdout.decode(), "stderr": result.stderr.decode(), "started_ns": started, "elapsed_ns": time.time_ns() - started}


def junit_counts(path, expected, since, expected_name=None):
    require(path.is_file() and not path.is_symlink(), "JUnit missing")
    require(path.stat().st_mtime_ns >= since, "stale JUnit")
    root = ET.parse(path).getroot()
    cases = list(root.iter("testcase"))
    executed = [c for c in cases if c.find("skipped") is None]
    require(len(executed) == expected, f"expected {expected} executed cases, received {len(executed)}")
    require(expected_name is None or any(c.get("name") == expected_name for c in executed), "required test absent")
    require(len(cases) >= expected, f"expected {expected} cases, received {len(cases)}")
    require(not any(list(c.iter("rerunFailure")) or list(c.iter("flakyFailure")) for c in cases), "retry evidence")
    return {"cases": len(cases), "failures": len(list(root.iter("failure"))),
            "errors": len(list(root.iter("error"))), "skipped": len(list(root.iter("skipped")))}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--source-sha", required=True)
    p.add_argument("--expected-tests", type=int, required=True)
    p.add_argument("--filter", required=True, help="nextest filter expression; always includes lifecycle in discovery")
    p.add_argument("--qualification-root", type=Path, required=True, help="owned root containing the exclusive CARGO_TARGET_DIR")
    p.add_argument("--expected-test-name", help="exact required JUnit testcase name (required for a one-test run)")
    p.add_argument("--workspace", action="store_true", help="discover full workspace instead of worker crate")
    a = p.parse_args()
    receipt = {"schemaVersion": 1, "execution": "not_started", "commands": [], "source": a.source_sha}
    out = a.output.absolute()
    created = False
    try:
        root = Path(output(["git", "rev-parse", "--show-toplevel"])).resolve()
        require(not out.exists() and not out.resolve().is_relative_to(root), "output must be new and outside checkout")
        require(a.expected_tests > 0, "expected-tests must be positive")
        require(a.expected_tests != 1 or a.expected_test_name, "one-test run requires expected-test-name")
        target = Path(os.environ.get("CARGO_TARGET_DIR", ""))
        require(target.is_absolute() and a.qualification_root.is_absolute(), "explicit absolute CARGO_TARGET_DIR and qualification-root required")
        require(target.resolve().is_relative_to(a.qualification_root.resolve()) and target.resolve() != a.qualification_root.resolve(), "target must be inside owned qualification-root")
        require(target.absolute() == target.resolve(), "symlink target path refused")
        out.mkdir(parents=True)
        created = True
        require(os.environ.get("HEAVY_GUARD_HELD") == "1", "run through heavy guard")
        require(platform.system() == "Darwin", "Darwin required")
        require(output(["sysctl", "-n", "hw.optional.arm64"]) == "1", "Apple Silicon required")
        require(output(["arch", "-x86_64", "sysctl", "-n", "sysctl.proc_translated"]) == "1", "Rosetta required")
        require(output(["git", "rev-parse", "HEAD"]) == a.source_sha, "source SHA mismatch")
        require(not output(["git", "status", "--porcelain=v1", "-uall"]), "checkout must be clean")
        require(output(["cargo", "nextest", "--version"]).startswith("cargo-nextest 0.9.140 "), "nextest 0.9.140 required")
        rust = output(["rustc", "-vV"])
        require("host: x86_64-apple-darwin" in rust, "x64 Rust toolchain required")
        receipt["tools"] = {}
        for tool in ("cargo", "rustc", "cargo-nextest"):
            executable = Path(shutil.which(tool) or "missing").resolve()
            receipt["tools"][tool] = {"path": str(executable), "architecture": x64(executable), **identity(executable)}
        receipt["rustc"] = rust
        receipt["config_sha256"] = {str(f.relative_to(root)): sha(f) for f in
                                    [root / ".config/nextest.toml", root / "Cargo.lock", root / "rust-toolchain.toml"]}
        receipt["host"] = output(["sw_vers"])
        def run(argv, name):
            receipt["commands"].append(argv)
            with (out / (name + ".stdout")).open("wb") as stdout, (out / (name + ".stderr")).open("wb") as stderr:
                rc = subprocess.run(argv, stdout=stdout, stderr=stderr).returncode
            require(rc == 0, f"{name} failed: {rc}; retained first output")
            return out / (name + ".stdout")
        cargo_file = run(["cargo", "metadata", "--locked", "--format-version", "1"], "cargo-metadata")
        scope = ["--workspace"] if a.workspace else ["-p", "zfb-compiler-worker"]
        binaries_file = run(["cargo", "nextest", "list", "--locked", *scope, "--list-type", "binaries-only", "--message-format", "json"], "binaries-metadata")
        binary_data = json.loads(binaries_file.read_text())
        require(Path(binary_data["rust-build-meta"]["target-directory"]).resolve() == target.resolve(), "metadata does not use exclusive target")
        host = host_from_metadata(binary_data, json.loads(cargo_file.read_text()))
        test_paths = [Path(v["binary-path"]) for v in binary_data["rust-binaries"].values()]
        test_identities = {str(f): identity(f) for f in test_paths}
        for f in test_paths:
            x64(f)
        receipt["test_binaries"] = test_identities
        receipt["host_binary"] = str(host)
        receipt["host_architecture"] = x64(host)
        before = receipt["before_warm"] = identity(host)
        receipt["metadata_sha256"] = {str(f.name): sha(f) for f in [cargo_file, binaries_file]}
        config = out / "junit.toml"
        report = out / "junit.xml"
        config.write_text('[profile.default.junit]\npath = ' + json.dumps(str(report)) + '\nstore-success-output = true\nstore-failure-output = true\n')
        receipt["commands"].append([str(host)])
        receipt["warm"] = warm(host, before)
        receipt["before_run"] = identity(host)
        require(receipt["before_run"] == before, "host replaced after warm")
        argv = ["cargo", "nextest", "run", "--cargo-metadata", str(cargo_file), "--binaries-metadata", str(binaries_file), "--profile", "default", "--retries", "0", "--no-fail-fast", "--tool-config-file", "rosetta:" + str(config), "-E", a.filter]
        require({str(f): identity(f) for f in test_paths} == test_identities, "test binaries changed before run")
        argv = ["/usr/bin/arch", "-x86_64", "/bin/sh", "-c", 'translated=$(sysctl -n sysctl.proc_translated); printf "rosetta_launcher_proc_translated=%s\n" "$translated" >&2; test "$translated" = 1 || exit 125; exec "$@"', "rosetta-nextest", *argv]
        receipt["commands"].append(argv)
        receipt["execution"] = "started"
        since = time.time_ns()
        with (out / "run.stdout").open("wb") as stdout, (out / "run.stderr").open("wb") as stderr:
            rc = subprocess.run(argv, stdout=stdout, stderr=stderr).returncode
        receipt["child_exit"] = rc
        require({str(f): identity(f) for f in test_paths} == test_identities, "test binaries changed during run")
        receipt["after_run"] = identity(host)
        require(receipt["after_run"] == before, "host changed during run")
        receipt["junit"] = junit_counts(report, a.expected_tests, since, a.expected_test_name)
        receipt["junit_sha256"] = sha(report)
        require(output(["git", "rev-parse", "HEAD"]) == a.source_sha and not output(["git", "status", "--porcelain=v1", "-uall"]), "source changed")
        receipt["execution"] = "captured"
        require(rc == 0 and receipt["junit"]["failures"] == 0 and receipt["junit"]["errors"] == 0, "tests failed; first result retained")
        return 0
    except (Refusal, OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError, ET.ParseError) as e:
        receipt["error"] = str(e)
        print(str(e))
        return receipt.get("child_exit") or 2
    finally:
        if created:
            (out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    raise SystemExit(main())
