#!/usr/bin/env python3
"""One #4302 platform-ledger row for the opt-in direct-V8 CLI.

Run on the native host for --target, from a clean checkout of the candidate
source. It records source/toolchain/binary/ICU identity, the binary's own
architecture and dynamic library dependencies, the target's resolved
dependency closure, and the actual CLI build/dev lifecycle observer (on
Windows its shutdown is a console Ctrl-C instead of SIGINT). It never downloads, dispatches or uploads anything.

With --build it runs the documented cargo build itself after the source check,
so the measured binary is tied to the verified checkout (wrap it in the heavy
guard with --guard). Without --build a supplied binary is not tied to the
source and the row is at most PARTIAL.

A host that does not natively match --target refuses with exit 3: a mismatched
interpreter host, Rosetta, a detected QEMU user-mode host, or a binary whose
header names another architecture. QEMU detection is best effort (CPU vendor in
/proc/cpuinfo); the recorded cpuinfo lets a reviewer check it. Every check it
cannot run or finish is NOT RUN/TIMEOUT and makes the row PARTIAL, never PASS.

Exit codes: 0 PASS, 1 FAIL or PARTIAL, 2 usage refusal, 3 not a native row.

--selection picks the build: `direct-v8` (default) is `--no-default-features
--features direct-v8`; `default` is the CLI's default features with no flags.
The selection is recorded in row.json. Before the default engine flips, the
`default` graph check resolves the Deno-backed default and FAILS honestly.

Usage (from the repository root):
  python3 prototypes/direct-v8/qualify-target.py --target aarch64-apple-darwin --build \
    --source-sha "$(git rev-parse HEAD)" --output target/direct-v8-qualify-darwin-arm64
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SELECTIONS = ("direct-v8", "default")
# (platform.system(), normalized machine) that natively executes each shipping triple.
NATIVE_HOSTS = {
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("Linux", "arm64"),
    "aarch64-apple-darwin": ("Darwin", "arm64"),
    "x86_64-apple-darwin": ("Darwin", "x86_64"),
    "x86_64-pc-windows-msvc": ("Windows", "x86_64"),
}
# Executable format and architecture each triple's binary header must declare.
BINARY_FORMATS = {
    "x86_64-unknown-linux-gnu": ("ELF", "x86_64"),
    "aarch64-unknown-linux-gnu": ("ELF", "arm64"),
    "aarch64-apple-darwin": ("Mach-O", "arm64"),
    "x86_64-apple-darwin": ("Mach-O", "x86_64"),
    "x86_64-pc-windows-msvc": ("PE", "x86_64"),
}
MACHINE_ALIASES = {"amd64": "x86_64", "x64": "x86_64", "aarch64": "arm64"}
ROLLDOWN_MARKERS = (b"zfb_rolldown_prototype", b"zfb-rolldown-prototype")
LIFECYCLE_TIMEOUT = 1800  # above check-cli-lifecycle.py's summed internal deadlines (~420 s)


def selection_args(selection, extra_features=()):
    """Exact cargo feature arguments for a selection (same contract as record-graph.py) and its label."""
    if selection == "default":
        return [], "default"
    features = ",".join(["direct-v8", *sorted(set(extra_features))])
    return ["--no-default-features", "--features", features], features


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def text(value):
    return value.decode(errors="replace") if isinstance(value, bytes) else value


def run(command, timeout=600, **kwargs):
    command = [str(part) for part in command]
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, **kwargs)
    except subprocess.TimeoutExpired as error:
        return {"command": command, "timeout": timeout, "stdout": text(error.stdout), "stderr": text(error.stderr)}
    except OSError as error:
        return {"command": command, "error": repr(error)}
    return {"command": command, "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


def cpuinfo():
    try:
        return Path("/proc/cpuinfo").read_text(errors="replace")
    except OSError:
        return None


def default_binary(target):
    return ROOT / "target/debug" / ("zfb.exe" if "windows" in target else "zfb")


def host_identity():
    machine = platform.machine().lower()
    system = platform.system()
    translated = None
    if system == "Darwin":
        probe = run(["sysctl", "-n", "sysctl.proc_translated"])
        translated = (probe.get("stdout") or "").strip() == "1"
    info = cpuinfo() if system == "Linux" else None
    return {"system": system, "machine": MACHINE_ALIASES.get(machine, machine), "raw_machine": machine,
            "release": platform.release(), "rosetta_translated": translated, "python": sys.version,
            "cpuinfo_head": "\n".join(info.splitlines()[:30]) if info else None}


def native_refusal(target, host):
    expected = NATIVE_HOSTS[target]
    if (host["system"], host["machine"]) != expected:
        return f"host {host['system']}/{host['machine']} is not native {expected[0]}/{expected[1]} for {target}"
    if host.get("rosetta_translated"):
        return "process runs under Rosetta translation; not a native row"
    info = host.get("cpuinfo_head") or ""
    if host["machine"] == "arm64" and any(vendor in info for vendor in ("GenuineIntel", "AuthenticAMD")):
        return "arm64 process reports an x86 CPU vendor; QEMU user-mode emulation is not a native row"
    return None


def binary_architecture(data):
    """Return (format, arch) from an executable header, or (None, None)."""
    if data[:4] == b"\x7fELF" and len(data) >= 20:
        endian = "<" if data[5] == 1 else ">"
        machine = struct.unpack(endian + "H", data[18:20])[0]
        return "ELF", {62: "x86_64", 183: "arm64"}.get(machine, f"e_machine={machine}")
    if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe") and len(data) >= 8:
        cputype = struct.unpack("<I", data[4:8])[0]
        return "Mach-O", {0x01000007: "x86_64", 0x0100000C: "arm64"}.get(cputype, f"cputype={cputype:#x}")
    if data[:4] in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
        return "Mach-O-universal", "multiple"
    if data[:2] == b"MZ" and len(data) >= 64:
        offset = struct.unpack("<I", data[60:64])[0]
        if data[offset:offset + 4] == b"PE\0\0":
            machine = struct.unpack("<H", data[offset + 4:offset + 6])[0]
            return "PE", {0x8664: "x86_64", 0xAA64: "arm64"}.get(machine, f"machine={machine:#x}")
    return None, None


def dynamic_dependencies(binary, system):
    tools = {"Linux": [["ldd", binary]], "Darwin": [["otool", "-L", binary]],
             "Windows": [["dumpbin", "/dependents", binary], ["objdump", "-p", binary]]}[system]
    for command in tools:
        if shutil.which(command[0]):
            result = run(command)
            if command[0] == "objdump" and "stdout" in result:
                # `objdump -p` prints every PE header and import table entry
                # (145 MB for a debug zfb.exe); the import DLL names are the row.
                result["stdout"] = "".join(
                    line.strip() + "\n" for line in result["stdout"].splitlines() if "DLL Name:" in line)
            return {**result, "passed": result.get("exit") == 0}
    return {"status": "NOT RUN", "passed": None, "reason": f"none of {[c[0] for c in tools]} is installed"}


def icu_module():
    spec = importlib.util.spec_from_file_location("prepare_icu", HERE / "prepare-icu.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def status_of(checks):
    verdicts = [check.get("passed") for check in checks.values()]
    return "FAIL" if False in verdicts else "PARTIAL" if None in verdicts else "PASS"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--target", required=True, choices=sorted(NATIVE_HOSTS))
    parser.add_argument("--binary", type=Path, help="Prebuilt CLI; default with --build: target/debug/zfb (zfb.exe on Windows)")
    parser.add_argument("--build", action="store_true", help="Run the documented cargo build after the source check")
    parser.add_argument("--guard", type=Path, help="heavy-guard.sh wrapping the --build command")
    parser.add_argument("--source-sha", required=True, help="Expected 40-hex candidate source commit")
    parser.add_argument("--output", required=True, type=Path, help="New directory for this row")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--selection", choices=SELECTIONS, default="direct-v8",
                        help="direct-v8: --no-default-features --features direct-v8; default: no feature flags")
    parser.add_argument("--rolldown", action="store_true",
                        help="Qualify direct-v8,rolldown-prototype and run with ZFB_ROLLDOWN_PROTOTYPE=1")
    args = parser.parse_args()
    if args.rolldown and args.selection != "direct-v8":
        parser.error("--rolldown extends the direct-v8 selection; it cannot combine with --selection default")
    if not args.binary and not args.build:
        parser.error("--binary is required without --build")
    output = args.output.resolve()
    if output.exists():
        parser.error(f"--output {output} already exists; every row needs a new directory")
    output.mkdir(parents=True)
    feature_args, features = selection_args(args.selection, ["rolldown-prototype"] if args.rolldown else [])
    binary = (args.binary or default_binary(args.target)).resolve()
    host = host_identity()
    build_command = [args.cargo, "build", "--locked", "-p", "zfb", *feature_args, "--bin", "zfb"]
    row = {"target": args.target, "host": host, "expected_source_sha": args.source_sha,
           "selection": args.selection, "features": features,
           "build_command": build_command, "binary_provenance": "built-by-harness" if args.build else "supplied",
           "checks": {}, "status": "FAIL"}
    checks = row["checks"]

    def save():
        (output / "row.json").write_text(json.dumps(row, indent=2) + "\n")

    def refuse(reason):
        row.update(status="NOT RUN", reason=reason)
        save()
        print("NOT RUN:", reason)
        raise SystemExit(3)

    refusal = native_refusal(args.target, host)
    if refusal:
        refuse(refusal)
    head = (run(["git", "rev-parse", "HEAD"], cwd=ROOT).get("stdout") or "").strip()
    dirty = run(["git", "status", "--porcelain", "--untracked-files=all"], cwd=ROOT).get("stdout")
    checks["source"] = {"head": head, "porcelain": dirty,
                        "passed": head == args.source_sha and dirty is not None and not dirty.strip()}
    checks["toolchain"] = {"rustc": run(["rustc", "-vV"], cwd=ROOT), "cargo": run([args.cargo, "-V"], cwd=ROOT)}
    checks["toolchain"]["passed"] = all(checks["toolchain"][name].get("exit") == 0 for name in ("rustc", "cargo"))
    if args.build:
        if not checks["source"]["passed"]:
            checks["build"] = {"status": "NOT RUN", "passed": False, "reason": "source check failed"}
            save()
            raise SystemExit(1)
        command = ["bash", args.guard, "--", *build_command] if args.guard else build_command
        build = run(command, timeout=None, cwd=ROOT)
        (output / "build.log").write_text((build.get("stdout") or "") + (build.get("stderr") or ""))
        checks["build"] = {"command": build["command"], "exit": build.get("exit"), "log": "build.log",
                           "passed": build.get("exit") == 0}
    else:
        checks["build"] = {"status": "NOT RUN", "passed": None,
                           "reason": "supplied binary is not tied to the verified source; rerun with --build"}
    if not binary.is_file():
        checks["binary"] = {"passed": False, "reason": f"missing {binary}"}
        save()
        raise SystemExit(1)
    data = binary.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    found = binary_architecture(data)
    if found != BINARY_FORMATS[args.target]:
        row["checks"]["binary"] = {"path": str(binary), "sha256": digest, "header": found}
        refuse(f"binary header {found} is not {BINARY_FORMATS[args.target]} for {args.target}")
    version = run([binary, "-V"])
    checks["binary"] = {"path": str(binary), "sha256": digest, "size": len(data), "header": found,
                        "version": version, "passed": version.get("exit") == 0}
    has_rolldown = any(marker in data for marker in ROLLDOWN_MARKERS)
    checks["feature_markers"] = {"rolldown_prototype_present": has_rolldown, "expected": args.rolldown,
                                 "passed": has_rolldown == args.rolldown}
    icu_pin = icu_module()
    icu = ROOT / icu_pin.VENDORED
    if icu.is_file():
        actual = sha256(icu)
        checks["icu"] = {"path": str(icu_pin.VENDORED), "sha256": actual, "pinned_sha256": icu_pin.DATA_SHA256,
                         "passed": actual == icu_pin.DATA_SHA256}
    else:
        checks["icu"] = {"status": "NOT RECORDED", "passed": None, "reason": f"{icu_pin.VENDORED} missing"}
    checks["dynamic_dependencies"] = dynamic_dependencies(binary, host["system"])
    graph = run([sys.executable, HERE / "record-graph.py", "--cli", "--target", args.target, "--cargo", args.cargo,
                 "--selection", args.selection, *(["--extra-feature", "rolldown-prototype"] if args.rolldown else []),
                 "--output", output / "graph.json"], cwd=ROOT)
    checks["graph"] = {**graph, "report": "graph.json", "passed": graph.get("exit") == 0}
    env = {key: value for key, value in os.environ.items() if key != "ZFB_ROLLDOWN_PROTOTYPE"}
    if args.rolldown:
        env["ZFB_ROLLDOWN_PROTOTYPE"] = "1"
    lifecycle = run([sys.executable, HERE / "check-cli-lifecycle.py", "--binary", binary, "--sha256", digest,
                     "--output", output / "lifecycle"], timeout=LIFECYCLE_TIMEOUT, cwd=ROOT, env=env)
    timed_out = "timeout" in lifecycle
    checks["lifecycle"] = {**lifecycle, "env": {"ZFB_ROLLDOWN_PROTOTYPE": env.get("ZFB_ROLLDOWN_PROTOTYPE")},
                           "status": "TIMEOUT (harness limit, not a behavioral result)" if timed_out else None,
                           "passed": None if timed_out else lifecycle.get("exit") == 0}
    checks["binary_unchanged"] = {"passed": binary.is_file() and sha256(binary) == digest}
    row["status"] = status_of(checks)
    save()
    print(row["status"], args.target, digest, {name: check.get("passed") for name, check in checks.items()})
    raise SystemExit(0 if row["status"] == "PASS" else 1)


if __name__ == "__main__":
    main()
