#!/usr/bin/env python3
"""One #4302 platform-ledger row for an already built opt-in direct-V8 CLI.

Run on the native host for --target, from a clean checkout of the candidate
source. It records source/toolchain/binary/ICU identity, the binary's dynamic
library dependencies, the target's resolved dependency closure, and (POSIX
only) the actual CLI build/dev lifecycle observer. It never builds, downloads,
dispatches or uploads anything.

A host that does not natively match --target refuses: cross-compiled,
emulated (Rosetta/QEMU) and saved-byte replays are not native rows. Every check
it cannot run is written as NOT RUN with a reason, never as a pass.

Usage (from the repository root):
  python3 prototypes/direct-v8/qualify-target.py --target aarch64-apple-darwin \
    --binary target/debug/zfb --source-sha <40-hex> --output target/direct-v8-qualify-darwin-arm64
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
# (platform.system(), normalized machine) that natively executes each shipping triple.
NATIVE_HOSTS = {
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("Linux", "arm64"),
    "aarch64-apple-darwin": ("Darwin", "arm64"),
    "x86_64-apple-darwin": ("Darwin", "x86_64"),
    "x86_64-pc-windows-msvc": ("Windows", "x86_64"),
}
MACHINE_ALIASES = {"amd64": "x86_64", "x64": "x86_64", "aarch64": "arm64"}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run(command, **kwargs):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=600, **kwargs)
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": [str(part) for part in command], "error": repr(error)}
    return {"command": [str(part) for part in command], "exit": result.returncode,
            "stdout": result.stdout, "stderr": result.stderr}


def host_identity():
    machine = platform.machine().lower()
    system = platform.system()
    translated = None
    if system == "Darwin":
        probe = run(["sysctl", "-n", "sysctl.proc_translated"])
        translated = probe.get("stdout", "").strip() == "1"
    return {"system": system, "machine": MACHINE_ALIASES.get(machine, machine), "raw_machine": machine,
            "release": platform.release(), "rosetta_translated": translated, "python": sys.version}


def native_refusal(target, host):
    expected = NATIVE_HOSTS[target]
    if (host["system"], host["machine"]) != expected:
        return f"host {host['system']}/{host['machine']} is not native {expected[0]}/{expected[1]} for {target}"
    if host["rosetta_translated"]:
        return "process runs under Rosetta translation; not a native row"
    return None


def dynamic_dependencies(binary, system):
    tools = {"Linux": [["ldd", binary]], "Darwin": [["otool", "-L", binary]],
             "Windows": [["dumpbin", "/dependents", binary], ["objdump", "-p", binary]]}[system]
    for command in tools:
        if shutil.which(command[0]):
            return {"status": "RECORDED", **run(command)}
    return {"status": "NOT RUN", "reason": f"none of {[c[0] for c in tools]} is installed"}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--target", required=True, choices=sorted(NATIVE_HOSTS))
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source-sha", required=True, help="Expected 40-hex candidate source commit")
    parser.add_argument("--output", required=True, type=Path, help="New directory for this row")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--rolldown", action="store_true",
                        help="Binary was built with direct-v8,rolldown-prototype; run it with ZFB_ROLLDOWN_PROTOTYPE=1")
    args = parser.parse_args()
    features = "direct-v8,rolldown-prototype" if args.rolldown else "direct-v8"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    host = host_identity()
    row = {"target": args.target, "host": host, "expected_source_sha": args.source_sha,
           "features": features, "build_command": [args.cargo, "build", "--locked", "-p", "zfb",
           "--no-default-features", "--features", features, "--bin", "zfb"], "checks": {}, "status": "FAIL"}
    checks = row["checks"]

    def save():
        (output / "row.json").write_text(json.dumps(row, indent=2) + "\n")

    refusal = native_refusal(args.target, host)
    if refusal:
        row.update(status="NOT RUN", reason=refusal)
        save()
        print("NOT RUN:", refusal)
        raise SystemExit(3)
    head = run(["git", "rev-parse", "HEAD"], cwd=ROOT).get("stdout", "").strip()
    dirty = run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT).get("stdout", "")
    checks["source"] = {"head": head, "clean": not dirty.strip(),
                        "passed": head == args.source_sha and not dirty.strip()}
    checks["toolchain"] = {"rustc": run(["rustc", "-vV"], cwd=ROOT), "cargo": run([args.cargo, "-V"], cwd=ROOT)}
    if not binary.is_file():
        checks["binary"] = {"passed": False, "reason": f"missing {binary}"}
        save()
        raise SystemExit(1)
    digest = sha256(binary)
    version = run([binary, "-V"])
    checks["binary"] = {"path": str(binary), "sha256": digest, "size": binary.stat().st_size, "version": version,
                        "passed": version.get("exit") == 0}
    icu = os.environ.get("ZFB_V8_ICU_DATA")
    checks["icu"] = ({"path": icu, "sha256": sha256(icu)} if icu and Path(icu).is_file()
                     else {"status": "NOT RECORDED", "reason": "ZFB_V8_ICU_DATA unset or missing"})
    checks["dynamic_dependencies"] = dynamic_dependencies(binary, host["system"])
    graph = run([sys.executable, HERE / "record-graph.py", "--cli", "--target", args.target, "--cargo", args.cargo,
                 *(["--extra-feature", "rolldown-prototype"] if args.rolldown else []),
                 "--output", output / "graph.json"], cwd=ROOT)
    checks["graph"] = {"exit": graph.get("exit"), "report": "graph.json", "passed": graph.get("exit") == 0}
    if os.name == "posix":
        env = {key: value for key, value in os.environ.items() if key != "ZFB_ROLLDOWN_PROTOTYPE"}
        if args.rolldown:
            env["ZFB_ROLLDOWN_PROTOTYPE"] = "1"
        row["lifecycle_env"] = {"ZFB_ROLLDOWN_PROTOTYPE": env.get("ZFB_ROLLDOWN_PROTOTYPE")}
        lifecycle = run([sys.executable, HERE / "check-cli-lifecycle.py", "--binary", binary, "--sha256", digest,
                         "--output", output / "lifecycle"], cwd=ROOT, env=env)
        checks["lifecycle"] = {"exit": lifecycle.get("exit"), "stdout": lifecycle.get("stdout"),
                               "passed": lifecycle.get("exit") == 0}
    else:
        checks["lifecycle"] = {"status": "NOT RUN", "passed": None,
                               "reason": "check-cli-lifecycle.py uses POSIX process groups/SIGINT; no Windows observer yet"}
    checks["binary_unchanged"] = {"passed": binary.is_file() and sha256(binary) == digest}
    verdicts = [check.get("passed") for check in checks.values() if "passed" in check]
    row["status"] = ("FAIL" if False in verdicts else "PARTIAL" if None in verdicts else "PASS")
    save()
    print(row["status"], args.target, digest, {name: check.get("passed") for name, check in checks.items()})
    raise SystemExit(0 if row["status"] == "PASS" else 1)


if __name__ == "__main__":
    main()
