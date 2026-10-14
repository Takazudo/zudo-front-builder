#!/usr/bin/env python3
"""Characterize the unchanged normal CLI's typed config boundary (CFG-08).

This is a baseline-only observer, not candidate CLI integration. Only the
ephemeral fixture directory is normalized by the Rust example.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

CASES = [
    ("object", "{}", "dist"),
    ("empty-array", "[]", "dist"),
    ("outdir-array", "['array-output']", "array-output"),
    ("wrong-array", "[42]", ["[0]", "expected path string"]),
    ("date", "new Date('2020-01-02T03:04:05Z')", ["expected struct Config"]),
    ("typed-field", "{port:'not-a-port'}", ["port:", "expected u16"]),
    ("presets", "{presets:[{outDir:'preset-output'}],outDir:'user-output'}", "user-output"),
]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    requests = [{"name": name, "source": "export default " + source + ";"}
                for name, source, _ in CASES]
    proc = subprocess.run([str(args.runner.resolve())], text=True,
                          input="".join(json.dumps(r) + "\n" for r in requests),
                          capture_output=True, timeout=60, check=False)
    prefix = "ZFB_OUTER_CONFIG_RECEIPT "
    receipts = [json.loads(line[len(prefix):]) for line in proc.stdout.splitlines()
                if line.startswith(prefix)]
    rows = []
    for (name, _, expected), receipt in zip(CASES, receipts):
        passed = receipt["case"] == name
        if isinstance(expected, str):
            passed &= receipt.get("ok") is True and receipt.get("outDir") == expected
        else:
            passed &= receipt.get("ok") is False and all(x in receipt.get("error", "") for x in expected)
        rows.append({"name": name, "expected": expected, "passed": passed, "receipt": receipt})
    passed = proc.returncode == 0 and len(rows) == len(CASES) and all(row["passed"] for row in rows)
    report = {"observer": "normal CLI outer loader (Deno default)",
              "runner_sha256": hashlib.sha256(args.runner.read_bytes()).hexdigest(),
              "requests": requests, "exit_code": proc.returncode,
              "stderr": proc.stderr, "passed": passed, "rows": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print("outer config", "PASS" if passed else "FAIL", sum(r["passed"] for r in rows), "/", len(CASES))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
