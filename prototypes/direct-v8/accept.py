#!/usr/bin/env python3
"""Reproducible phase-1 acceptance entry point for the direct-V8 prototype.

One command prepares the checksum-pinned ICU data, explicitly builds the
baseline (deno_core) and candidate (direct V8) runners with their exact feature
selections, runs the native library tests, verifies each executable's engine
receipt and SHA-256 immediately before the gates, and then runs the existing
selected acceptance gates against those same bytes. Every step, exit code,
duration, executable digest and gate count is written to <output>/acceptance.json.

Refusals (the run fails, nothing is reported as passed):
  - a missing executable, a runner that exits 2 because its feature is off
    (the `cargo build --workspace --all-targets` trap overwrites the candidate
    example with its feature-disabled form), a runner whose receipt names the
    wrong engine, or a runner whose probe value is wrong;
  - an executable whose bytes change while the gates run;
  - any gate that fails; there is no baseline/Node/Deno fallback executor.

The Rust commands and gates run through the heavy guard when --guard names one.
This is Linux x86_64 native evidence only; it proves nothing about other
platforms, the normal CLI, dev reload or live-host cancellation beyond what
the invoked gates themselves assert. Python only orchestrates and compares.

Usage (from the repository root):
  python3 prototypes/direct-v8/accept.py --output target/direct-v8-accept-1 \
    [--guard $HOME/.claude/scripts/heavy-guard.sh] [--icu-archive <deno_core_icudata .crate>] \
    [--generated --esbuild <esbuild 0.25.12> --chromium <browser> [--node <node>] [--rustc <rustc>]]
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time

# Never leave __pycache__ behind in the checkout: the producer records the
# working tree's status, and a stray cache directory would read as dirty.
sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
RECEIPT_PREFIX = "ZFB_HOST_RECEIPT "
BASELINE_ENGINE = "deno_core 0.399.0 / v8 147.4.0"
CANDIDATE_ENGINE = "direct v8 147.4.0"
# The probe both runners must answer identically; a fake that only echoes an
# engine string without evaluating JavaScript fails the value comparison.
PROBE_SOURCE = "export default { probe: 6 * 7, text: 'direct-v8-accept' };"
PROBE_VALUE = {"probe": 42, "text": "direct-v8-accept"}
BASELINE_BUILD = ["cargo", "build", "--locked", "-p", "zfb-render", "--example", "direct_v8_baseline"]
CANDIDATE_BUILD = ["cargo", "build", "--locked", "-p", "zfb-js-host", "--no-default-features",
                   "--features", "direct-v8", "--example", "direct_v8"]
CANDIDATE_TESTS = ["cargo", "test", "--locked", "-p", "zfb-js-host", "--no-default-features",
                   "--features", "direct-v8", "--lib"]
# The shared-services unit tests (limits pins, dispatch mode, shims) compile
# only with `services`; `cargo test -p zfb-js-host` alone runs none of them.
SERVICES_TESTS = ["cargo", "test", "--locked", "-p", "zfb-js-host", "--features", "services"]
OUTER_BUILD = ["cargo", "build", "--locked", "-p", "zfb", "--example", "direct_v8_outer_config"]
POSTPROCESSOR_BUILD = ["cargo", "build", "--locked", "-p", "zfb-build", "--no-default-features",
                       "--example", "direct_v8_postprocess"]
ZFB_BUILD = ["cargo", "build", "--locked", "-p", "zfb", "--bin", "zfb"]
WRAPPER_TEST_BUILD = ["cargo", "test", "--locked", "-p", "zfb", "--lib", "--no-run",
                      "--message-format=json"]
RECORDED_ENV = ("RUSTUP_TOOLCHAIN", "CARGO_BUILD_JOBS", "CARGO_INCREMENTAL", "CARGO_TARGET_DIR",
                "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG", "ZFB_V8_ICU_DATA",
                "ZFB_ESBUILD_BIN", "ZFB_NODE_BIN")


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Refusal(Exception):
    """An acceptance precondition failed; nothing after it is evidence."""


def parse_receipts(stdout):
    return [json.loads(line[len(RECEIPT_PREFIX):]) for line in stdout.splitlines()
            if line.startswith(RECEIPT_PREFIX)]


def verify_runner(path, expected_engine, label, timeout=60):
    """Return {path, sha256, engine, receipt} or raise Refusal.

    Refuses a missing or non-executable file, a feature-disabled runner (exit
    2 naming its feature), a wrong engine identity, a failed probe or a wrong
    probe value. The digest is taken before the probe runs.
    """
    path = Path(path)
    if not path.is_file():
        raise Refusal(f"{label}: missing executable {path}")
    if not os.access(path, os.X_OK):
        raise Refusal(f"{label}: {path} is not executable")
    digest = sha(path)
    request = json.dumps({"kind": "config", "source": PROBE_SOURCE}) + "\n"
    try:
        proc = subprocess.run([str(path)], input=request, text=True, capture_output=True,
                              timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise Refusal(f"{label}: engine probe watchdog expired after {timeout}s") from error
    if proc.returncode == 2 and "requires the" in proc.stderr and "feature" in proc.stderr:
        raise Refusal(f"{label}: {path} is the feature-disabled form of the runner "
                      f"({proc.stderr.strip()}); rebuild it with its explicit feature command")
    receipts = parse_receipts(proc.stdout)
    if proc.returncode != 0 or len(receipts) != 1:
        raise Refusal(f"{label}: engine probe failed (exit {proc.returncode}, "
                      f"{len(receipts)} receipt(s)); stderr: {proc.stderr.strip()[:400]}")
    receipt = receipts[0]
    if receipt.get("engine") != expected_engine:
        raise Refusal(f"{label}: {path} reports engine {receipt.get('engine')!r}, "
                      f"not {expected_engine!r}; this is the wrong executable")
    if receipt.get("ok") is not True or json.dumps(receipt.get("value"), sort_keys=True) != \
            json.dumps(PROBE_VALUE, sort_keys=True):
        raise Refusal(f"{label}: engine probe returned {receipt!r}, not the expected value")
    if sha(path) != digest:
        raise Refusal(f"{label}: {path} changed while it was being probed")
    return {"path": str(path), "sha256": digest, "engine": receipt["engine"], "receipt": receipt}


def parse_test_result(stdout):
    """`cargo test` summary -> (passed, failed) or None when absent."""
    match = re.search(r"test result: (\w+)\. (\d+) passed; (\d+) failed;", stdout)
    if not match:
        return None
    return {"status": match.group(1), "passed": int(match.group(2)), "failed": int(match.group(3))}


def discover_wrapper_test_binary(json_lines, package="zfb"):
    """The lib test executable among `cargo test --no-run --message-format=json` lines."""
    found = []
    for line in json_lines.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact" or not message.get("executable"):
            continue
        target = message.get("target", {})
        if target.get("kind") == ["lib"] and target.get("name") == package and \
                message.get("profile", {}).get("test"):
            found.append(message["executable"])
    if len(found) != 1:
        raise Refusal(f"expected exactly one {package} lib test executable, found {found}")
    return found[0]


def load_icu_module():
    spec = importlib.util.spec_from_file_location("prepare_icu", HERE / "prepare-icu.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Acceptance:
    def __init__(self, args):
        self.args = args
        self.out = args.output.resolve()
        self.logs = self.out / "logs"
        self.steps = []
        self.env = os.environ.copy()

    def run(self, label, argv, *, guarded=False, env=None, cwd=ROOT, timeout=7200, stdin=None):
        command = [str(x) for x in argv]
        if guarded and self.args.guard:
            command = ["bash", str(self.args.guard), "--", *command]
        start = time.monotonic()
        merged = self.env | (env or {})
        try:
            proc = subprocess.run(command, cwd=cwd, env=merged, input=stdin, text=True,
                                  capture_output=True, timeout=timeout, check=False)
        except subprocess.TimeoutExpired as error:
            self.steps.append({"label": label, "argv": command, "cwd": str(cwd), "guarded": guarded,
                               "watchdog_seconds": timeout, "passed": False})
            raise Refusal(f"{label}: watchdog expired after {timeout}s; a watchdog is a failed "
                          "observation, never clean completion") from error
        (self.logs / f"{label}.stdout").write_text(proc.stdout)
        (self.logs / f"{label}.stderr").write_text(proc.stderr)
        step = {"label": label, "argv": command, "cwd": str(cwd), "guarded": guarded,
                "exit_code": proc.returncode, "seconds": round(time.monotonic() - start, 1),
                "env": {name: merged[name] for name in RECORDED_ENV if name in merged}}
        self.steps.append(step)
        if proc.returncode != 0:
            raise Refusal(f"{label} failed (exit {proc.returncode}); see {self.logs}/{label}.stderr")
        return proc

    def cargo_target_dir(self):
        proc = self.run("cargo-metadata-target", ["cargo", "metadata", "--locked", "--no-deps",
                                                  "--format-version", "1"])
        return Path(json.loads(proc.stdout)["target_directory"])

    def prepare_icu(self):
        icu = load_icu_module()
        argv = [sys.executable, "-I", "-B", HERE / "prepare-icu.py"]
        if self.args.icu_archive:
            argv += ["--archive", self.args.icu_archive]
        proc = self.run("prepare-icu", argv)
        path = Path(proc.stdout.strip().splitlines()[-1])
        digest = sha(path)
        if digest != icu.DATA_SHA256:
            raise Refusal(f"prepared ICU data {path} hashes to {digest}, not the pinned {icu.DATA_SHA256}")
        self.env["ZFB_V8_ICU_DATA"] = str(path)
        return {"path": str(path), "sha256": digest, "archive_sha256_pin": icu.ARCHIVE_SHA256}

    def environment(self):
        def out(argv):
            try:
                return subprocess.run(argv, capture_output=True, text=True, check=False,
                                      env=self.env, cwd=ROOT).stdout.strip()
            except OSError as error:
                return f"unavailable: {error}"
        return {
            "commit": out(["git", "rev-parse", "HEAD"]),
            "status_porcelain": out(["git", "status", "--porcelain=v1"]),
            "rustc": out(["rustc", "-vV"]),
            "cargo": out(["cargo", "--version"]),
            "python": sys.version,
            "platform": platform.platform(),
            "machine": platform.machine(),
            "env": {name: self.env[name] for name in RECORDED_ENV if name in self.env},
        }

    def gate(self, label, argv, output, counts, report_path=None):
        """Run one existing checker; `counts(report)` summarizes its JSON.

        `output` is the checker's `--output` argument (a file, or for
        check-generated.py a new directory); `report_path` names the JSON it
        writes when that differs from `output`.
        """
        argv = [sys.executable, "-I", "-B", HERE / argv[0], *argv[1:], "--output", output]
        passed = True
        try:
            self.run(label, argv, guarded=True)
        except Refusal:
            passed = False
        report_path = Path(report_path or output)
        report = json.loads(report_path.read_text()) if report_path.is_file() else None
        summary = {"output": str(report_path), "passed": passed and report_passed(report),
                   "counts": counts(report) if report is not None else None}
        return summary


def report_passed(report):
    """Whether a checker's JSON report records a pass.

    check-http.py, check-outer-config.py and check-generated.py write a
    top-level `passed`; check-config.py and check-modules.py write one
    `passed` per engine under `results`, and every engine must have passed.
    """
    if not isinstance(report, dict):
        return False
    if "passed" in report:
        return report["passed"] is True
    results = report.get("results")
    return bool(results) and all(isinstance(r, dict) and r.get("passed") is True for r in results)


def engine_counts(report):
    return [{"engine": r.get("engine"), "executable_sha256": r.get("executable_sha256"),
             "receipts": r.get("receipt_count"), "expected": r.get("expected_count"),
             "rows_passed": sum(1 for row in r.get("rows", []) if row.get("passed")),
             "passed": r.get("passed")} for r in report.get("results", [])]


def rows_counts(report):
    rows = report.get("rows", [])
    return {"rows": len(rows), "rows_passed": sum(1 for row in rows if row.get("passed")),
            "passed": report.get("passed")}


def generated_counts(report):
    result = {"passed": report.get("passed"), "snapshot_manifest_sha256": report.get("snapshot_manifest_sha256")}
    for side in ("baseline", "candidate"):
        rows = report.get(side, {}).get("rows", [])
        result[side] = {"packets": len(rows), "packets_passed": sum(1 for r in rows if r.get("passed"))}
    parity = report.get("byte_and_value_parity", [])
    result["parity_rows"] = len(parity)
    result["parity_passed"] = sum(1 for r in parity if r.get("passed"))
    result["browser"] = report.get("browser", {}).get("passed")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output", type=Path, required=True, help="New directory below repository target/")
    parser.add_argument("--guard", type=Path, help="heavy-guard.sh to wrap Rust builds and gates")
    parser.add_argument("--icu-archive", type=Path, help="Already fetched deno_core_icudata .crate")
    parser.add_argument("--generated", action="store_true", help="Also run the generated-input gate")
    parser.add_argument("--esbuild", type=Path, help="Pinned esbuild binary (generated gate)")
    parser.add_argument("--chromium", type=Path, help="Installed browser executable (generated gate)")
    parser.add_argument("--node", type=Path, help="Node executable (generated gate); default: PATH")
    parser.add_argument("--rustc", type=Path, help="rustc executable (generated gate); default: rustup which rustc")
    args = parser.parse_args()
    out = args.output.resolve()
    if not out.is_relative_to(ROOT / "target") or out == ROOT / "target" or out.exists():
        parser.error("--output must be a new directory below this repository's ignored target/")
    if args.generated and not (args.esbuild and args.chromium):
        parser.error("--generated needs --esbuild and --chromium")
    out.mkdir(parents=True)
    acceptance = Acceptance(args)
    acceptance.logs.mkdir()
    report = {"schema": 1, "passed": False, "steps": acceptance.steps, "limitations": [
        "Linux x86_64 native evidence only; other platforms untested.",
        "Standalone runners prove host behavior, not candidate CLI/factory/dev-reload integration.",
        "A watchdog expiry is a failed observation, never clean shutdown evidence.",
        "No baseline, Node or Deno fallback executor is selected when the candidate is missing."]}
    try:
        report["environment"] = acceptance.environment()
        report["icu"] = acceptance.prepare_icu()
        target = acceptance.cargo_target_dir()
        report["target_directory"] = str(target)
        # The services tests build the crate's examples without `direct-v8`,
        # which overwrites examples/direct_v8 with its feature-disabled form
        # (the same trap as an all-targets build), so they run BEFORE the
        # explicit candidate build; run 4 of this entry point was refused by its
        # own verifier for exactly that reason.
        services = acceptance.run("services-tests", SERVICES_TESTS, guarded=True)
        services_summary = parse_test_result(services.stdout)
        if not services_summary or services_summary["status"] != "ok" or services_summary["passed"] == 0 \
                or services_summary["failed"]:
            raise Refusal(f"shared-services tests did not pass with a nonzero count: {services_summary}")
        report["services_tests"] = services_summary
        acceptance.run("build-baseline", BASELINE_BUILD, guarded=True)
        acceptance.run("build-outer-config", OUTER_BUILD, guarded=True)
        acceptance.run("build-candidate", CANDIDATE_BUILD, guarded=True)
        tests = acceptance.run("candidate-lib-tests", CANDIDATE_TESTS, guarded=True)
        summary = parse_test_result(tests.stdout)
        if not summary or summary["status"] != "ok" or summary["passed"] == 0 or summary["failed"]:
            raise Refusal(f"candidate library tests did not pass with a nonzero count: {summary}")
        report["candidate_lib_tests"] = summary
        examples = target / "debug" / "examples"
        runners = {
            "baseline": verify_runner(examples / "direct_v8_baseline", BASELINE_ENGINE, "baseline"),
            "candidate": verify_runner(examples / "direct_v8", CANDIDATE_ENGINE, "candidate"),
        }
        outer = examples / "direct_v8_outer_config"
        if not outer.is_file():
            raise Refusal(f"missing outer-config runner {outer}")
        runners["outer_config"] = {"path": str(outer), "sha256": sha(outer)}
        report["executables"] = runners
        baseline, candidate = runners["baseline"]["path"], runners["candidate"]["path"]

        def unchanged(after):
            for name in ("baseline", "candidate"):
                now = sha(runners[name]["path"])
                if now != runners[name]["sha256"]:
                    raise Refusal(f"{name} executable bytes changed during the {after} gate "
                                  f"({runners[name]['sha256']} -> {now}); rebuild and rerun")

        gates = {}
        gates["config"] = acceptance.gate("gate-config", ["check-config.py", "--baseline", baseline,
                                          "--candidate", candidate], out / "config-gate.json", engine_counts)
        unchanged("config")
        gates["services"] = acceptance.gate("gate-services", ["check-config.py", "--kind", "script",
                                            "--cases", HERE / "service-cases.json", "--baseline", baseline,
                                            "--candidate", candidate], out / "services-gate.json", engine_counts)
        unchanged("services")
        gates["http"] = acceptance.gate("gate-http", ["check-http.py", "--baseline", baseline,
                                        "--candidate", candidate], out / "http-gate.json", rows_counts)
        unchanged("http")
        gates["modules"] = acceptance.gate("gate-modules", ["check-modules.py", "--baseline", baseline,
                                           "--candidate", candidate, "--fixture-dir", out / "modules-fixtures"],
                                           out / "modules-gate.json", engine_counts)
        unchanged("modules")
        gates["outer_config"] = acceptance.gate("gate-outer-config", ["check-outer-config.py", "--runner", outer],
                                                out / "outer-config-gate.json", rows_counts)
        if args.generated:
            acceptance.run("build-postprocessor", POSTPROCESSOR_BUILD, guarded=True)
            zfb_build = acceptance.run("build-zfb", ZFB_BUILD, guarded=True,
                                       env={"ZFB_ESBUILD_BIN": str(args.esbuild.resolve())})
            wrapper_json = acceptance.run("build-wrapper-test", WRAPPER_TEST_BUILD, guarded=True,
                                          env={"ZFB_ESBUILD_BIN": str(args.esbuild.resolve())})
            wrapper = discover_wrapper_test_binary(wrapper_json.stdout)
            # The all-targets trap: re-verify the candidate after further cargo builds.
            unchanged("generated-prerequisite builds")
            node = args.node or shutil.which("node")
            rustc = args.rustc or subprocess.run(["rustup", "which", "rustc"], capture_output=True,
                                                  text=True, check=True, env=acceptance.env).stdout.strip()
            build_record = json.dumps({"argv": zfb_build.args if hasattr(zfb_build, "args") else ZFB_BUILD,
                                       "env": {k: acceptance.env[k] for k in RECORDED_ENV if k in acceptance.env},
                                       "ZFB_ESBUILD_BIN": str(args.esbuild.resolve())}, sort_keys=True)
            postprocessor = examples / "direct_v8_postprocess"
            runners["postprocessor"] = {"path": str(postprocessor), "sha256": sha(postprocessor)}
            runners["zfb"] = {"path": str(target / "debug" / "zfb"), "sha256": sha(target / "debug" / "zfb")}
            runners["wrapper_test_binary"] = {"path": wrapper, "sha256": sha(wrapper)}
            generated = out / "generated"
            acceptance.run("prepare-fixtures", [sys.executable, "-I", "-B", HERE / "prepare-fixtures.py",
                           "--zfb", target / "debug" / "zfb", "--esbuild", args.esbuild.resolve(),
                           "--wrapper-test-binary", wrapper, "--rustc", rustc, "--node", node,
                           "--producer-build-command", build_record, "--output", generated], guarded=True)
            gates["generated"] = acceptance.gate("gate-generated", ["check-generated.py", "--snapshot",
                                                 generated / "frozen", "--baseline", baseline, "--candidate",
                                                 candidate, "--postprocessor", postprocessor, "--node", node,
                                                 "--chromium", args.chromium.resolve()],
                                                 out / "generated-results", generated_counts,
                                                 report_path=out / "generated-results" / "results.json")
            unchanged("generated")
        report["gates"] = gates
        report["executables_after"] = {name: sha(info["path"]) for name, info in runners.items()}
        report["passed"] = all(g["passed"] for g in gates.values())
        if not report["passed"]:
            failed = [name for name, g in gates.items() if not g["passed"]]
            report["error"] = f"gates failed: {', '.join(failed)}"
    except Refusal as error:
        report["error"] = str(error)
        report["passed"] = False
    (out / "acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
    print("direct-v8 acceptance", "PASS" if report["passed"] else "FAIL", out / "acceptance.json")
    if report.get("error"):
        print(report["error"], file=sys.stderr)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
