#!/usr/bin/env python3
"""Record the selected candidate dependency closure, never the whole lockfile."""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    selection = ["--locked", "-p", "zfb-js-host", "--no-default-features", "--features", "direct-v8"]
    command = [args.cargo, "tree", *selection, "--target", "x86_64-unknown-linux-gnu",
               "--edges", "normal,build", "--prefix", "none", "--format", "{p} features=[{f}]"]
    tree = subprocess.check_output(command, text=True)
    rows = sorted(set(line.removesuffix(" (*)") for line in tree.splitlines()))
    forbidden = {"deno_core", "deno_ops", "deno_error", "serde_v8", "zfb-render", "zfb-build"}
    invalid = [row for row in rows if row.split()[0] in forbidden or row.split()[0].startswith("deno_")]
    engines = [row for row in rows if row.startswith("v8 ")]
    passed = not invalid and len(engines) == 1 and engines[0].startswith("v8 v147.4.0 ")
    report = {"command": command, "target": "x86_64-unknown-linux-gnu",
              "executable_target": "example:direct_v8", "passed": passed,
              "forbidden_dependencies": invalid, "engines": engines, "packages_and_features": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print("PASS" if passed else "FAIL", len(rows), "resolved package/feature rows", engines)
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
