#!/usr/bin/env python3
"""Characterize the unchanged normal CLI's typed config boundary (CFG-08).

This is a baseline-only observer, not candidate CLI integration. Only the
ephemeral fixture directory is normalized by the Rust example.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import sys


# -I excludes sibling imports. Load only this checkout's known helper.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("runner_protocol", Path(__file__).with_name("runner-protocol.py"))
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)

CASES = [
    ("object", "{}", "dist"),
    ("empty-array", "[]", "dist"),
    ("outdir-array", "['array-output']", "array-output"),
    ("wrong-array", "[42]", ["[0]", "expected path string"]),
    ("date", "new Date('2020-01-02T03:04:05Z')", ["expected struct Config"]),
    ("typed-field", "{port:'not-a-port'}", ["port:", "expected u16"]),
    ("presets", "{presets:[{outDir:'preset-output'}],outDir:'user-output'}", "user-output"),
]


def run(runner, *, timeout=60, engine=None):
    requests = [{"name": name, "source": "export default " + source + ";"}
                for name, source, _ in CASES]
    result = protocol.run_receipts(runner, "".join(json.dumps(r) + "\n" for r in requests),
                                   "ZFB_OUTER_CONFIG_RECEIPT ", len(CASES), timeout=timeout)
    receipts = result["receipts"]
    rows = []
    for index, (name, _, expected) in enumerate(CASES):
        receipt = receipts[index] if index < len(receipts) else None
        failures = []
        if not isinstance(receipt, dict):
            failures.append("missing or invalid receipt")
        else:
            if engine is not None and receipt.get("engine") != engine:
                failures.append("compiled engine identity differs")
            if receipt.get("case") != name:
                failures.append("case name or order differs")
            if isinstance(expected, str):
                if receipt.get("ok") is not True or receipt.get("outDir") != expected:
                    failures.append("expected successful outDir differs")
            elif receipt.get("ok") is not False or not isinstance(receipt.get("error"), str):
                failures.append("expected failure with a string error")
            elif not all(fragment in receipt["error"] for fragment in expected):
                failures.append("expected error fragments differ")
        rows.append({"name": name, "expected": expected, "passed": not failures,
                     "failures": failures, "receipt": receipt})
    result.update(observer="actual CLI outer loader: " + (engine or "Deno default"),
                  runner_sha256=result["executable_sha256"], requests=requests, rows=rows,
                  passed=result["passed"] and all(row["passed"] for row in rows))
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--engine", choices=["direct v8 147.4.0", "deno_core 0.399.0 / v8 147.4.0"])
    args = parser.parse_args()
    interrupt = None
    try:
        report = run(args.runner, engine=args.engine)
    except BaseException as error:
        # A cancelled check still leaves a failed report, then propagates (#4289 U7).
        interrupt = error
        report = {"passed": False}
    protocol.finish_report(report, interrupt)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    if interrupt is not None:
        raise interrupt
    print("outer config", "PASS" if report["passed"] else "FAIL",
          sum(r["passed"] for r in report["rows"]), "/", len(CASES))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
