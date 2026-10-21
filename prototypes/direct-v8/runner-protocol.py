"""Bounded config-checker transport; no JavaScript execution or fallback.

Keep one receipt slot per framed line, even when that line is invalid. This
prevents a malformed result from shifting later results onto the wrong case.
Watchdogs are failed observations, not proof of native or child-tree cleanup.
"""
import base64
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import subprocess


MAX_JSON_DEPTH = 128

# -I excludes sibling imports. Load only this checkout's known helper.
_spec = importlib.util.spec_from_file_location("watchdog_drain", Path(__file__).with_name("watchdog-drain.py"))
drain = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(drain)
PENDING = drain.PendingRegistry()
# The bounded drain is POSIX only (U8); Windows keeps kill + communicate.
USE_DRAIN = os.name == "posix"


def finish_report(report, interrupt=None):
    """Record an interrupt and unreaped children in a checker report (U4, U7).

    Call once at checker exit, then write the report and re-raise the interrupt.
    Children are polled, never waited for; one still alive fails the report.
    """
    if interrupt is not None:
        report["passed"] = False
        report["error"] = f"interrupted: {type(interrupt).__name__}: {interrupt}"
        if getattr(interrupt, "watchdog_cleanup", None) is not None:
            report["watchdog_cleanup"] = interrupt.watchdog_cleanup
    report["released_children"] = PENDING.release()
    if any(entry["released_to_os"] for entry in report["released_children"]):
        report["passed"] = False
    return report


def parse_json(source):
    """Reject ambiguous/non-finite JSON and nesting unsafe for report output."""
    def object_pairs(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError(f"duplicate JSON key: {key!r}")
            value[key] = item
        return value

    def invalid_constant(value):
        raise ValueError(f"non-finite JSON number: {value}")

    def finite_float(value):
        number = float(value)
        if not math.isfinite(number):
            invalid_constant(value)
        return number

    try:
        value = json.loads(source, object_pairs_hook=object_pairs,
                           parse_constant=invalid_constant, parse_float=finite_float)
    except RecursionError as error:
        raise ValueError("JSON nesting exceeds decoder capacity") from error
    # Decoders can accept more nesting than the report encoder can serialize.
    # Leave ample depth for report wrappers and retain rejected raw wire bytes.
    pending = [(value, 1)]
    while pending:
        item, depth = pending.pop()
        if isinstance(item, (dict, list)):
            if depth > MAX_JSON_DEPTH:
                raise ValueError(f"JSON nesting exceeds {MAX_JSON_DEPTH} levels")
            children = item.values() if isinstance(item, dict) else item
            pending.extend((child, depth + 1) for child in children)
    return value


def run_receipts(binary, wire, prefix, expected_count, *, timeout=60, cleanup_timeout=1):
    """Run one independent process and retain protocol/operational failures.

    A valid transport result still needs the checker's independent semantic
    assertions. Digests bind this invocation, not the authenticity of a runner.
    On POSIX the watchdog kills, drains and reaps only the direct child within
    one shared cleanup_timeout; a cancellation propagates with this result
    attached as exception.watchdog_cleanup.
    """
    binary = Path(binary).resolve()
    failures = []
    result = {"executable": str(binary), "executable_sha256": None,
              "exit_code": None, "stdout": "", "stderr": "",
              "receipt_count": 0, "expected_count": expected_count,
              "receipts": [], "failures": failures, "passed": False}

    def digest(when):
        try:
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise OSError("missing or non-executable runner")
            with binary.open("rb") as stream:
                return hashlib.file_digest(stream, "sha256").hexdigest()
        except OSError as error:
            failures.append(f"runner {when}: {type(error).__name__}: {error}")
            return None

    def save_stream(name, data):
        data = data or b""
        try:
            result[name] = data.decode("utf-8")
        except UnicodeDecodeError:
            failures.append(f"runner {name} is not UTF-8")
            result[name] = data.decode("utf-8", errors="replace")
            result[name + "_base64"] = base64.b64encode(data).decode("ascii")

    if type(expected_count) is not int or expected_count <= 0:
        failures.append("expected receipt count must be a positive integer")
        return result
    result["executable_sha256"] = digest("before execution")
    if failures:
        return result
    stdout, stderr = b"", b""
    cancellation = None
    if not USE_DRAIN:
        try:
            process = subprocess.run([str(binary)], input=wire.encode("utf-8"),
                                     capture_output=True, timeout=timeout, check=False)
            stdout, stderr = process.stdout, process.stderr
            result["exit_code"] = process.returncode
            if process.returncode != 0:
                failures.append(f"runner exited unsuccessfully ({process.returncode})")
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout, error.stderr
            result["watchdog"] = f"runner watchdog expired after {timeout}s; no cleanup claim"
            failures.append(result["watchdog"])
        except OSError as error:
            failures.append(f"runner failed to start: {type(error).__name__}: {error}")
    else:
        drained = None
        try:
            drained = drain.run([str(binary)], wire.encode("utf-8"), timeout=timeout,
                                cleanup_timeout=cleanup_timeout)
        except BaseException as error:
            drained = getattr(error, "watchdog_cleanup", None)
            if not isinstance(error, OSError):
                if drained is None:
                    raise
                # Cancellation or a defect: record it, then propagate it (U7).
                cancellation = error
                failures.append(f"runner interrupted: {type(error).__name__}: {error}")
            elif drained is not None:
                failures.append(f"runner communication failed: {type(error).__name__}: {error}")
            else:
                failures.append(f"runner failed to start: {type(error).__name__}: {error}")
        if drained is not None:
            stdout, stderr = drained["stdout"], drained["stderr"]
            cleanup = PENDING.project(drained)
            if drained["watchdog"]:
                result["watchdog"] = (f"runner watchdog expired after {timeout}s; "
                                      "direct child only, no descendant cleanup claim")
                failures.append(result["watchdog"])
            elif drained["exit_code"] is not None:
                result["exit_code"] = drained["exit_code"]
                if drained["exit_code"] != 0:
                    failures.append(f"runner exited unsuccessfully ({drained['exit_code']})")
            # drain_complete says whether partial output is complete or best effort.
            result["watchdog_cleanup"] = {key: cleanup[key] for key in (
                "drain_complete", "cleanup_exit_code", "cleanup_errors",
                "pending_child_pid", "pending_stream_count")}
            result["watchdog_cleanup"].update(execution_budget_seconds=cleanup["execution_seconds"],
                                              cleanup_budget_seconds=cleanup["cleanup_seconds"])
            failures.extend(f"runner cleanup: {message}" for message in cleanup["cleanup_errors"])
    save_stream("stdout", stdout)
    save_stream("stderr", stderr)
    if cancellation is not None:
        # Propagate promptly: no post-run digest or receipt parsing after an interrupt.
        cancellation.watchdog_cleanup = result
        raise cancellation
    result["executable_sha256_after"] = digest("after execution")
    if result["executable_sha256_after"] is not None and \
            result["executable_sha256_after"] != result["executable_sha256"]:
        failures.append("runner executable bytes changed during execution")
    # Only LF frames the protocol. Unicode line separators can occur inside JSON strings.
    frames = [line[len(prefix):] for line in result["stdout"].split("\n")
              if line.startswith(prefix)]
    result["receipt_count"] = len(frames)
    if len(frames) != expected_count:
        failures.append(f"expected {expected_count} framed receipts, received {len(frames)}")
    for index, frame in enumerate(frames):
        receipt = None
        try:
            receipt = parse_json(frame)
            if not isinstance(receipt, dict):
                raise ValueError("receipt must be a JSON object")
        except ValueError as error:
            failures.append(f"receipt {index + 1}: {error}")
            receipt = None
        result["receipts"].append(receipt)
    result["passed"] = not failures
    return result
