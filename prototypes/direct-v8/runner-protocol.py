"""Bounded config-checker transport; no JavaScript execution or fallback.

Keep one receipt slot per framed line, even when that line is invalid. This
prevents a malformed result from shifting later results onto the wrong case.
Watchdogs are failed observations, not proof of native or child-tree cleanup.
"""
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess


MAX_JSON_DEPTH = 128


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


def run_receipts(binary, wire, prefix, expected_count, *, timeout=60):
    """Run one independent process and retain protocol/operational failures.

    A valid transport result still needs the checker's independent semantic
    assertions. Digests bind this invocation, not the authenticity of a runner.
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
    save_stream("stdout", stdout)
    save_stream("stderr", stderr)
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
