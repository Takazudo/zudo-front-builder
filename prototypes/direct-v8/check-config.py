#!/usr/bin/env python3
"""Execute frozen config inputs in independent native runner processes.

Python only orchestrates/compares; it never evaluates JavaScript. A watchdog
failure is recorded as failure, never successful isolate cleanup.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys


# -I excludes sibling imports. Load only this checkout's known helper.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("runner_protocol", Path(__file__).with_name("runner-protocol.py"))
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)


def category(message):
    if message == "default export must be a plain object":
        return "not_a_plain_object"
    if message.startswith("config module load error:"):
        return "module_load"
    if message.startswith("config evaluation error:"):
        return "evaluation"
    if "bare imports and node:* specifiers" in message:
        return "bare_import_rejected"
    return "unknown"


def run(binary, engine, cases, kind="config"):
    wire = "".join(json.dumps({"kind": kind, "source": c["source"],
                              **({"mode": c["mode"]} if "mode" in c else {})}) + "\n" for c in cases)
    result = protocol.run_receipts(binary, wire, "ZFB_HOST_RECEIPT ", len(cases))
    receipts = result["receipts"]
    rows = []
    for index, case in enumerate(cases):
        receipt = receipts[index] if index < len(receipts) else None
        expected = case["expected"]
        failures = []
        if not isinstance(receipt, dict):
            failures.append("missing or invalid receipt")
        else:
            if receipt.get("engine") != engine:
                failures.append("unexpected engine identity")
            if receipt.get("ok") is not expected["ok"]:
                failures.append("success/error differs or is not a boolean")
            elif expected["ok"]:
                # Missing is not null; Python bool/int equality is not JSON equality.
                if "value" not in receipt or json.dumps(receipt["value"], sort_keys=True) != \
                        json.dumps(expected["value"], sort_keys=True):
                    failures.append("literal value differs or is missing")
            else:
                error = receipt.get("error")
                if not isinstance(error, str):
                    failures.append("error must be a string")
                else:
                    if "category" in expected and category(error) != expected["category"]:
                        failures.append("error category differs")
                    for fragment in expected.get("contains", expected.get("errorContains", [])):
                        if fragment not in error:
                            failures.append("missing error fragment: " + fragment)
        rows.append({"name": case["name"], "contracts": case["contracts"],
                     "passed": not failures, "failures": failures, "receipt": receipt})
    result.update(engine=engine, rows=rows,
                  passed=result["passed"] and all(row["passed"] for row in rows))
    return result


def validate_cases(cases):
    """Reject an empty or unusable oracle before starting either engine."""
    if not isinstance(cases, list) or not cases:
        raise ValueError("config corpus must be a nonempty array")
    names = set()
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError("config case must be an object")
        name = case.get("name")
        if not isinstance(name, str) or not name or name in names:
            raise ValueError("config case names must be nonempty and unique")
        names.add(name)
        if not isinstance(case.get("source"), str) or "contracts" not in case:
            raise ValueError(f"{name}: source and contracts are required")
        expected = case.get("expected")
        if not isinstance(expected, dict) or type(expected.get("ok")) is not bool:
            raise ValueError(f"{name}: expected.ok must be a boolean")
        if expected["ok"] and "value" not in expected:
            raise ValueError(f"{name}: successful expectation needs a value")
        for key in ("contains", "errorContains"):
            if key in expected and (not isinstance(expected[key], list) or
                                    any(not isinstance(item, str) for item in expected[key])):
                raise ValueError(f"{name}: {key} must be a string array")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--kind", choices=["config", "script"], default="config")
    parser.add_argument("--cases", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    corpus = args.cases or Path(__file__).with_name("config-cases.json")
    report = {"passed": False, "results": []}
    interrupt = None
    try:
        corpus_bytes = corpus.read_bytes()
        report["corpus_sha256"] = hashlib.sha256(corpus_bytes).hexdigest()
        cases = protocol.parse_json(corpus_bytes)
        validate_cases(cases)
        report["results"].append(run(args.baseline, "deno_core 0.399.0 / v8 147.4.0", cases, args.kind))
        if args.candidate:
            report["results"].append(run(args.candidate, "direct v8 147.4.0", cases, args.kind))
        report["corpus_sha256_after"] = hashlib.sha256(corpus.read_bytes()).hexdigest()
        if report["corpus_sha256_after"] != report["corpus_sha256"]:
            raise ValueError("config corpus bytes changed during execution")
        report["passed"] = all(result["passed"] for result in report["results"])
    except BaseException as error:
        if isinstance(error, (OSError, ValueError)) and not hasattr(error, "watchdog_cleanup"):
            report["error"] = f"{type(error).__name__}: {error}"
        else:
            # A cancelled check still leaves a failed report, then propagates (#4289 U7).
            interrupt = error
    protocol.finish_report(report, interrupt)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    if interrupt is not None:
        raise interrupt
    for result in report["results"]:
        print(result["engine"], "PASS" if result["passed"] else "FAIL",
              sum(row["passed"] for row in result.get("rows", [])), "/", result["expected_count"])
        for row in result.get("rows", []):
            if not row["passed"]:
                print(row["name"], row["failures"], row["receipt"])
    if report.get("error"):
        print(report["error"], file=sys.stderr)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
