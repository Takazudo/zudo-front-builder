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
  - any recorded executable whose bytes change or which disappears/loses
    execute permission during subsequent builds, fixture preparation or gates;
  - any gate that fails or writes an unreadable/malformed report; there is no
    baseline/Node/Deno fallback executor.

Failed commands retain their logs and step metadata, including partial raw
watchdog output. Once a run's output directory is initialized, failures and
interruptions save acceptance.json (provided the output remains writable).
Unexpected harness bugs are recorded and re-raised, never converted to passes.

The Rust commands and gates run through the heavy guard when --guard names one.
This is Linux x86_64 native evidence only; it proves nothing about other
platforms, the normal CLI, dev reload or live-host cancellation beyond what
the invoked gates themselves assert. Python only orchestrates and compares.

Usage (from the repository root):
  python3 prototypes/direct-v8/accept.py --output target/direct-v8-accept-1 \
    [--guard $HOME/.claude/scripts/heavy-guard.sh] \
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
# direct-v8 is the default runtime since #4390, so the deno_core baseline must
# name its feature set; with defaults the example refuses to run (exit 2) and
# there is nothing to compare against.
BASELINE_FEATURES = ["--no-default-features", "--features", "embed_v8,syntect-onig"]
BASELINE_BUILD = ["cargo", "build", "--locked", "-p", "zfb-render", *BASELINE_FEATURES,
                  "--example", "direct_v8_baseline"]
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
                "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
                "ZFB_ESBUILD_BIN", "ZFB_NODE_BIN")


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Refusal(Exception):
    """An acceptance precondition failed; nothing after it is evidence."""


def record_executable(path, label):
    """Pin a runnable file, converting filesystem failures into refusals."""
    path = Path(path)
    try:
        if not path.is_file():
            raise Refusal(f"{label}: missing executable {path}")
        if not os.access(path, os.X_OK):
            raise Refusal(f"{label}: {path} is not executable")
        return {"path": str(path), "sha256": sha(path)}
    except OSError as error:
        raise Refusal(f"{label}: cannot read executable {path}: {error}") from error


def verify_executables_unchanged(runners, after):
    """Enforce every recorded digest, including producer/helper executables."""
    digests = {}
    for name, info in runners.items():
        now = record_executable(info["path"], f"{name} after {after}")["sha256"]
        digests[name] = now
        if now != info["sha256"]:
            raise Refusal(f"{name} executable bytes changed during {after} "
                          f"({info['sha256']} -> {now}); rebuild and rerun")
    return digests


def parse_receipts(stdout):
    receipts = []
    for line in stdout.splitlines():
        if not line.startswith(RECEIPT_PREFIX):
            continue
        try:
            receipt = json.loads(line[len(RECEIPT_PREFIX):])
        except ValueError as error:
            raise Refusal(f"invalid host receipt JSON: {error}") from error
        if not isinstance(receipt, dict):
            raise Refusal("host receipt must be a JSON object")
        receipts.append(receipt)
    return receipts


def verify_runner(path, expected_engine, label, timeout=60):
    """Return {path, sha256, engine, receipt} or raise Refusal.

    Refuses a missing or non-executable file, a feature-disabled runner (exit
    2 naming its feature), a wrong engine identity, a failed probe or a wrong
    probe value. The digest is taken before the probe runs.
    """
    recorded = record_executable(path, label)
    path = Path(recorded["path"])
    digest = recorded["sha256"]
    request = json.dumps({"kind": "config", "source": PROBE_SOURCE}) + "\n"
    try:
        proc = subprocess.run([str(path)], input=request, text=True, capture_output=True,
                              timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise Refusal(f"{label}: engine probe watchdog expired after {timeout}s") from error
    except OSError as error:
        raise Refusal(f"{label}: cannot execute engine probe {path}: {type(error).__name__}: {error}") from error
    if proc.returncode == 2 and "requires the" in proc.stderr and "feature" in proc.stderr:
        raise Refusal(f"{label}: {path} is the feature-disabled form of the runner "
                      f"({proc.stderr.strip()}); rebuild it with its explicit feature command")
    try:
        receipts = parse_receipts(proc.stdout)
    except Refusal as error:
        raise Refusal(f"{label}: {error}") from error
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
    if record_executable(path, label)["sha256"] != digest:
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

    def write_logs(self, label, stdout, stderr):
        # TimeoutExpired streams are bytes even with text=True, and can stop
        # midway through a UTF-8 sequence. Preserve them without decoding.
        for suffix, output in (("stdout", stdout), ("stderr", stderr)):
            data = output.encode("utf-8") if isinstance(output, str) else output
            (self.logs / f"{label}.{suffix}").write_bytes(data or b"")

    def run(self, label, argv, *, guarded=False, env=None, cwd=ROOT, timeout=7200, stdin=None):
        command = [str(x) for x in argv]
        if guarded and self.args.guard:
            command = ["bash", str(self.args.guard), "--", *command]
        start = time.monotonic()
        merged = self.env | (env or {})
        step = {"label": label, "argv": command, "cwd": str(cwd), "guarded": guarded,
                "exit_code": None, "passed": False, "watchdog_seconds": timeout,
                "env": {name: merged[name] for name in RECORDED_ENV if name in merged}}
        # Register before launching so even a spawn failure or interruption
        # remains an observation. No exit code is invented for an unobserved exit.
        self.steps.append(step)
        try:
            proc = subprocess.run(command, cwd=cwd, env=merged, input=stdin, text=True,
                                  capture_output=True, timeout=timeout, check=False)
        except subprocess.TimeoutExpired as error:
            step["error"] = (f"{label}: watchdog expired after {timeout}s; a watchdog is a failed "
                             "observation, never clean completion")
            self.write_logs(label, error.stdout, error.stderr)
            raise Refusal(step["error"]) from error
        except OSError as error:
            step["error"] = f"{label}: cannot execute {command[0]}: {type(error).__name__}: {error}"
            self.write_logs(label, None, None)
            raise Refusal(step["error"]) from error
        else:
            step["exit_code"] = proc.returncode
            self.write_logs(label, proc.stdout, proc.stderr)
            if proc.returncode != 0:
                step["error"] = f"{label} failed (exit {proc.returncode}); see {self.logs}/{label}.stderr"
                raise Refusal(step["error"])
            step["passed"] = True
            return proc
        finally:
            step["seconds"] = round(time.monotonic() - start, 1)

    def cargo_target_dir(self):
        proc = self.run("cargo-metadata-target", ["cargo", "metadata", "--locked", "--no-deps",
                                                  "--format-version", "1"])
        return Path(json.loads(proc.stdout)["target_directory"])

    def prepare_icu(self):
        icu = load_icu_module()
        proc = self.run("prepare-icu", [sys.executable, "-I", "-B", HERE / "prepare-icu.py", "--verify"])
        path = Path(proc.stdout.strip().splitlines()[-1])
        digest = sha(path)
        if digest != icu.DATA_SHA256:
            raise Refusal(f"prepared ICU data {path} hashes to {digest}, not the pinned {icu.DATA_SHA256}")
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
        errors = []
        try:
            self.run(label, argv, guarded=True)
        except Refusal as error:
            errors.append(str(error))
        report_path = Path(report_path or output)
        summary = {"output": str(report_path), "passed": False, "counts": None}
        try:
            report = json.loads(report_path.read_text(encoding="utf-8"))
            if not isinstance(report, dict):
                raise ValueError("gate report must be a JSON object")
            # Count extraction is part of validating the report's structure.
            # A passing flag cannot rescue missing or contradictory evidence.
            summary["counts"] = counts(report)
            if not report_passed(report):
                errors.append(f"{label}: gate report does not record a pass")
        except (OSError, ValueError, TypeError, AttributeError) as error:
            errors.append(f"{label}: invalid gate report {report_path}: {type(error).__name__}: {error}")
            summary["counts"] = None
        summary["passed"] = not errors
        if errors:
            summary["errors"] = errors
        return summary


def report_passed(report):
    """Read the reported pass flag; count callbacks validate the evidence too.

    check-http.py, check-outer-config.py and check-generated.py write a
    top-level `passed`; check-config.py and check-modules.py write one
    `passed` per engine under `results`, and every engine must have passed.
    """
    if not isinstance(report, dict):
        return False
    if "passed" in report:
        return report["passed"] is True
    results = report.get("results")
    return isinstance(results, list) and bool(results) and \
        all(isinstance(r, dict) and r.get("passed") is True for r in results)


def passed_row_names(rows, label):
    """Validate evidence beneath a claimed pass, never vacuous all([])."""
    if not isinstance(rows, list) or not rows:
        raise ValueError(f"{label}: expected nonempty rows")
    names = []
    for row in rows:
        if not isinstance(row, dict) or row.get("passed") is not True:
            raise ValueError(f"{label}: every row must explicitly pass")
        name = row.get("name")
        if not isinstance(name, str) or not name.strip() or name in names:
            raise ValueError(f"{label}: row names must be nonempty and unique")
        names.append(name)
    return names


def passed_exit(report, label):
    if type(report.get("exit_code")) is not int or report["exit_code"] != 0:
        raise ValueError(f"{label}: success requires an observed integer exit_code of zero")


def passed_engine_pair(results, label):
    if not isinstance(results, list) or len(results) != 2:
        raise ValueError(f"{label}: expected exactly two engine results")
    names = []
    for result in results:
        if not isinstance(result, dict) or result.get("passed") is not True:
            raise ValueError(f"{label}: every engine must explicitly pass")
        engine = result.get("engine")
        if engine not in (BASELINE_ENGINE, CANDIDATE_ENGINE) or engine in names:
            raise ValueError(f"{label}: expected one baseline and one candidate engine")
        passed_exit(result, f"{label} {engine}")
        names.append(engine)
    return results


def engine_counts(report):
    # Sparse failed reports (for example, a watchdog) retain their available
    # counts. A claimed success, however, must contain coherent paired evidence.
    if report_passed(report):
        cases = []
        for result in passed_engine_pair(report.get("results"), "engine results"):
            names = passed_row_names(result.get("rows"), result["engine"])
            expected, received = result.get("expected_count"), result.get("receipt_count")
            if type(expected) is not int or type(received) is not int or \
                    expected <= 0 or expected != received or expected != len(names):
                raise ValueError(f"{result['engine']}: expected, receipt and row counts must agree and be nonzero")
            cases.append(names)
        if cases[0] != cases[1]:
            raise ValueError("engine results: baseline and candidate case names/order differ")
    return [{"engine": r.get("engine"), "executable_sha256": r.get("executable_sha256"),
             "receipts": r.get("receipt_count"), "expected": r.get("expected_count"),
             "rows_passed": sum(1 for row in r.get("rows", []) if row.get("passed") is True),
             "passed": r.get("passed")} for r in report.get("results", [])]


def rows_counts(report):
    rows = report.get("rows", [])
    if report.get("passed") is True:
        passed_row_names(rows, "rows")
    return {"rows": len(rows), "rows_passed": sum(1 for row in rows if row.get("passed") is True),
            "passed": report.get("passed")}


def http_counts(report):
    result = rows_counts(report)
    if report.get("passed") is True:
        for row in report["rows"]:
            passed_engine_pair(row.get("results"), f"HTTP {row['name']}")
    return result


def outer_counts(report):
    result = rows_counts(report)
    if report.get("passed") is True:
        passed_exit(report, "outer config")
        requests = report.get("requests")
        if not isinstance(requests, list) or \
                any(not isinstance(request, dict) for request in requests) or \
                [request.get("name") for request in requests] != [row["name"] for row in report["rows"]]:
            raise ValueError("outer config: request and row names/order must agree")
    return result


def generated_counts(report):
    if report.get("passed") is True:
        names = []
        for side, engine in (("baseline", BASELINE_ENGINE), ("candidate", CANDIDATE_ENGINE)):
            result = report.get(side)
            if not isinstance(result, dict) or result.get("passed") is not True or result.get("engine") != engine:
                raise ValueError(f"generated {side}: expected an explicitly passing {engine} result")
            names.append(passed_row_names(result.get("rows"), f"generated {side}"))
        if names[0] != names[1] or "authored-mapped-throw" not in names[0]:
            raise ValueError("generated: engine case names/order must agree and include authored-mapped-throw")
        parity = passed_row_names(report.get("byte_and_value_parity"), "generated parity")
        if parity != [name for name in names[0] if name != "authored-mapped-throw"]:
            raise ValueError("generated: parity must cover every non-throw case in order")
        browser = report.get("browser")
        if not isinstance(browser, dict) or browser.get("passed") is not True or "not_run" in browser:
            raise ValueError("generated browser: expected an explicitly passing, executed browser check")
        passed_exit(browser, "generated browser")
        digest = report.get("snapshot_manifest_sha256")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("generated: missing or invalid snapshot manifest SHA-256")
    result = {"passed": report.get("passed"), "snapshot_manifest_sha256": report.get("snapshot_manifest_sha256")}
    for side in ("baseline", "candidate"):
        rows = report.get(side, {}).get("rows", [])
        result[side] = {"packets": len(rows), "packets_passed": sum(1 for r in rows if r.get("passed") is True)}
    parity = report.get("byte_and_value_parity", [])
    result["parity_rows"] = len(parity)
    result["parity_passed"] = sum(1 for r in parity if r.get("passed") is True)
    result["browser"] = report.get("browser", {}).get("passed")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output", type=Path, required=True, help="New directory below repository target/")
    parser.add_argument("--guard", type=Path, help="heavy-guard.sh to wrap Rust builds and gates")
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
    runners = {}
    gates = {}
    report["executables"] = runners
    report["gates"] = gates
    report["executable_checks"] = []

    def unchanged(after):
        check = {"after": after, "passed": False}
        report["executable_checks"].append(check)
        try:
            check["sha256"] = verify_executables_unchanged(runners, after)
        except Refusal as error:
            check["error"] = str(error)
            raise
        check["passed"] = True
        return check["sha256"]

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
        runners["baseline"] = verify_runner(examples / "direct_v8_baseline", BASELINE_ENGINE, "baseline")
        runners["candidate"] = verify_runner(examples / "direct_v8", CANDIDATE_ENGINE, "candidate")
        outer = examples / "direct_v8_outer_config"
        runners["outer_config"] = record_executable(outer, "outer_config")
        baseline, candidate = runners["baseline"]["path"], runners["candidate"]["path"]
        unchanged("runner probes")

        gates["config"] = acceptance.gate("gate-config", ["check-config.py", "--baseline", baseline,
                                          "--candidate", candidate], out / "config-gate.json", engine_counts)
        unchanged("config")
        gates["services"] = acceptance.gate("gate-services", ["check-config.py", "--kind", "script",
                                            "--cases", HERE / "service-cases.json", "--baseline", baseline,
                                            "--candidate", candidate], out / "services-gate.json", engine_counts)
        unchanged("services")
        gates["http"] = acceptance.gate("gate-http", ["check-http.py", "--baseline", baseline,
                                        "--candidate", candidate], out / "http-gate.json", http_counts)
        unchanged("http")
        gates["modules"] = acceptance.gate("gate-modules", ["check-modules.py", "--baseline", baseline,
                                           "--candidate", candidate, "--fixture-dir", out / "modules-fixtures"],
                                           out / "modules-gate.json", engine_counts)
        unchanged("modules")
        gates["outer_config"] = acceptance.gate("gate-outer-config", ["check-outer-config.py", "--runner", outer],
                                                out / "outer-config-gate.json", outer_counts)
        unchanged("outer-config")
        if args.generated:
            # Pin each build output immediately: a later Cargo command must not
            # silently replace an earlier helper before its first digest is read.
            acceptance.run("build-postprocessor", POSTPROCESSOR_BUILD, guarded=True)
            postprocessor = examples / "direct_v8_postprocess"
            runners["postprocessor"] = record_executable(postprocessor, "postprocessor")
            unchanged("postprocessor build")
            zfb_build = acceptance.run("build-zfb", ZFB_BUILD, guarded=True,
                                       env={"ZFB_ESBUILD_BIN": str(args.esbuild.resolve())})
            runners["zfb"] = record_executable(target / "debug" / "zfb", "zfb")
            unchanged("zfb build")
            wrapper_json = acceptance.run("build-wrapper-test", WRAPPER_TEST_BUILD, guarded=True,
                                          env={"ZFB_ESBUILD_BIN": str(args.esbuild.resolve())})
            wrapper = discover_wrapper_test_binary(wrapper_json.stdout)
            runners["wrapper_test_binary"] = record_executable(wrapper, "wrapper_test_binary")
            # The all-targets trap also applies to the already pinned helpers.
            unchanged("generated-prerequisite builds")
            node = args.node or shutil.which("node")
            rustc = args.rustc or subprocess.run(["rustup", "which", "rustc"], capture_output=True,
                                                  text=True, check=True, env=acceptance.env).stdout.strip()
            build_record = json.dumps({"argv": zfb_build.args if hasattr(zfb_build, "args") else ZFB_BUILD,
                                       "env": {k: acceptance.env[k] for k in RECORDED_ENV if k in acceptance.env},
                                       "ZFB_ESBUILD_BIN": str(args.esbuild.resolve())}, sort_keys=True)
            generated = out / "generated"
            acceptance.run("prepare-fixtures", [sys.executable, "-I", "-B", HERE / "prepare-fixtures.py",
                           "--zfb", target / "debug" / "zfb", "--esbuild", args.esbuild.resolve(),
                           "--wrapper-test-binary", wrapper, "--rustc", rustc, "--node", node,
                           "--producer-build-command", build_record, "--output", generated], guarded=True)
            unchanged("fixture preparation")
            gates["generated"] = acceptance.gate("gate-generated", ["check-generated.py", "--snapshot",
                                                 generated / "frozen", "--baseline", baseline, "--candidate",
                                                 candidate, "--postprocessor", postprocessor, "--node", node,
                                                 "--chromium", args.chromium.resolve()],
                                                 out / "generated-results", generated_counts,
                                                 report_path=out / "generated-results" / "results.json")
            unchanged("generated")
        report["executables_after"] = unchanged("final verification")
        report["passed"] = all(g["passed"] for g in gates.values())
        if not report["passed"]:
            failed = [name for name, g in gates.items() if not g["passed"]]
            report["error"] = f"gates failed: {', '.join(failed)}"
    except Refusal as error:
        report["error"] = str(error)
        report["passed"] = False
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = f"{type(error).__name__}: {error}"
        report["passed"] = False
    except BaseException as error:
        # Persist the failed observation, but preserve traceback/interrupt
        # semantics for unexpected bugs and cancellation. Never hide them.
        report["error"] = f"{type(error).__name__}: {error}"
        report["passed"] = False
        raise
    finally:
        (out / "acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
    print("direct-v8 acceptance", "PASS" if report["passed"] else "FAIL", out / "acceptance.json")
    if report.get("error"):
        print(report["error"], file=sys.stderr)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
