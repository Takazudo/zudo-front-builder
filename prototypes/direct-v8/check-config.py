#!/usr/bin/env python3
"""Execute frozen config inputs in independent native runner processes.

Python only orchestrates/compares; it never evaluates JavaScript. A watchdog
failure is recorded as failure, never successful isolate cleanup.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys


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


def run(binary, engine, cases):
    wire = "".join(json.dumps({"kind": "config", "source": c["source"]}) + "\n" for c in cases)
    try:
        proc = subprocess.run([str(binary.resolve())], input=wire, text=True,
                              capture_output=True, timeout=60, check=False)
    except subprocess.TimeoutExpired as exc:
        return {"engine": engine, "passed": False, "watchdog": str(exc)}
    receipts = [json.loads(line.removeprefix("ZFB_HOST_RECEIPT "))
                for line in proc.stdout.splitlines() if line.startswith("ZFB_HOST_RECEIPT ")]
    rows = []
    for case, receipt in zip(cases, receipts):
        expected = case["expected"]
        failures = []
        if receipt.get("engine") != engine:
            failures.append("unexpected engine identity")
        if receipt.get("ok") != expected["ok"]:
            failures.append("success/error differs")
        elif expected["ok"]:
            # JSON type matters: Python's bool == int must not weaken the oracle.
            if json.dumps(receipt.get("value"), sort_keys=True) != json.dumps(expected["value"], sort_keys=True):
                failures.append("literal value differs")
        else:
            error = receipt.get("error", "")
            if category(error) != expected["category"]:
                failures.append("error category differs")
            for fragment in expected["contains"]:
                if fragment not in error:
                    failures.append("missing error fragment: " + fragment)
        rows.append({"name": case["name"], "contracts": case["contracts"],
                     "passed": not failures, "failures": failures, "receipt": receipt})
    return {"engine": engine, "executable": str(binary.resolve()),
            "executable_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "exit_code": proc.returncode, "receipt_count": len(receipts),
            "expected_count": len(cases), "stderr": proc.stderr,
            "passed": proc.returncode == 0 and len(receipts) == len(cases) and all(r["passed"] for r in rows),
            "rows": rows}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    corpus = Path(__file__).with_name("config-cases.json")
    cases = json.loads(corpus.read_text())
    results = [run(args.baseline, "deno_core 0.399.0 / v8 147.4.0", cases)]
    if args.candidate:
        results.append(run(args.candidate, "direct v8 147.4.0", cases))
    report = {"corpus_sha256": hashlib.sha256(corpus.read_bytes()).hexdigest(), "results": results}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    for result in results:
        print(result["engine"], "PASS" if result["passed"] else "FAIL",
              sum(row["passed"] for row in result.get("rows", [])), "/", len(cases))
        for row in result.get("rows", []):
            if not row["passed"]:
                print(row["name"], row["failures"], row["receipt"])
    return 0 if all(result["passed"] for result in results) else 1


if __name__ == "__main__":
    sys.exit(main())
