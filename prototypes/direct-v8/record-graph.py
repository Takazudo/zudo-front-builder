#!/usr/bin/env python3
"""Record the selected candidate dependency closure, never the whole lockfile.

`--target` resolves the closure for any shipping triple without building, so a
graph row is resolution evidence only; native link/load needs a matching host.

`--selection direct-v8` (default) resolves `--no-default-features --features
direct-v8[,extra]`; `--selection default` resolves the CLI's default features
with no flags. The check is the same, so a default selection that still pulls
`deno_*`/`serde_v8` or a second V8 FAILS, and is reported as FAIL.
"""
import argparse
import json
from pathlib import Path
import subprocess

# Mirrors the release.yml build matrix (#4302 native target obligations).
SHIPPING_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
)
EXTRA_FEATURES = ("rolldown-prototype",)
SELECTIONS = ("direct-v8", "default")


def selection_args(selection, extra_features=()):
    """Exact cargo feature arguments for a selection, plus the features label recorded in reports."""
    if selection == "default":
        return [], "default"
    features = ",".join(["direct-v8", *sorted(set(extra_features))])
    return ["--no-default-features", "--features", features], features


def evaluate(rows, cli):
    # The CLI legitimately depends on zfb-render/zfb-build; only the standalone js-host graph forbids them.
    forbidden = {"deno_core", "deno_ops", "deno_error", "serde_v8"}
    if not cli:
        forbidden |= {"zfb-render", "zfb-build"}
    invalid = [row for row in rows if row.split()[0] in forbidden or row.split()[0].startswith("deno_")]
    engines = [row for row in rows if row.startswith("v8 ")]
    passed = not invalid and len(engines) == 1 and engines[0].startswith("v8 v147.4.0 ")
    return passed, invalid, engines


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cli", action="store_true", help="Inspect the actual opt-in zfb CLI graph")
    parser.add_argument("--target", default=SHIPPING_TARGETS[0], choices=SHIPPING_TARGETS)
    parser.add_argument("--extra-feature", action="append", default=[], choices=EXTRA_FEATURES,
                        help="Additional zfb CLI feature unified with direct-v8 (requires --cli)")
    parser.add_argument("--selection", choices=SELECTIONS, default="direct-v8",
                        help="direct-v8: --no-default-features --features direct-v8; default: no feature flags (requires --cli)")
    args = parser.parse_args()
    if args.extra_feature and not args.cli:
        parser.error("--extra-feature selects zfb CLI features and requires --cli")
    if args.selection == "default" and (not args.cli or args.extra_feature):
        parser.error("--selection default resolves the zfb CLI default features: requires --cli, no --extra-feature")
    feature_args, features = selection_args(args.selection, args.extra_feature)
    selection = ["--locked", "-p", "zfb" if args.cli else "zfb-js-host", *feature_args]
    # --color never: CARGO_TERM_COLOR=always (set on CI) wraps the " (*)" dedupe marker in escapes.
    command = [args.cargo, "tree", "--color", "never", *selection, "--target", args.target,
               "--edges", "normal,build", "--prefix", "none", "--format", "{p} features=[{f}]"]
    tree = subprocess.check_output(command, text=True)
    rows = sorted(set(line.removesuffix(" (*)") for line in tree.splitlines()))
    passed, invalid, engines = evaluate(rows, args.cli)
    report = {"command": command, "target": args.target, "selection": args.selection, "features": features,
              "evidence_kind": "dependency-resolution (no build, link or execution)",
              "executable_target": "bin:zfb" if args.cli else "example:direct_v8",
              "build_command": [args.cargo, "build", *selection, "--target", args.target,
                                *( ["--bin", "zfb"] if args.cli else ["--example", "direct_v8"] )], "passed": passed,
              "forbidden_dependencies": invalid, "engines": engines, "packages_and_features": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print("PASS" if passed else "FAIL", args.target, args.selection, features, len(rows), "resolved package/feature rows", engines)
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
