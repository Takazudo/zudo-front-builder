#!/usr/bin/env python3
"""Capture native exam provenance and reconcile nextest's inventories with results."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import xml.etree.ElementTree as ET


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(Path(path).read_text(), object_pairs_hook=unique)


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def metadata(source):
    require(re.fullmatch(r"[0-9a-f]{40}", source), "expected full lowercase source SHA")
    require(command("git", "rev-parse", "HEAD") == source, "checkout SHA mismatch")
    versions = {name: command(*args) for name, args in {
        "rust": ("rustc", "--version"), "cargo": ("cargo", "--version"),
        "node": ("node", "--version"), "pnpm": ("pnpm", "--version"),
        "npm": ("npm", "--version"), "nextest": ("cargo", "nextest", "--version"),
        "esbuild": ("crates/zfb/binaries/esbuild/esbuild", "--version"),
    }.items()}
    require(versions["rust"].startswith("rustc 1.99.1 "), "Rust must be 1.99.1")
    require(versions["node"] == "v22.22.2", "Node must be 22.22.2")
    require(versions["nextest"].split()[1] == "0.9.148", "nextest must be 0.9.148")
    require("pnpm@" + versions["pnpm"] == read_json("package.json")["packageManager"], "pnpm pin mismatch")
    pins = Path("crates/zfb-toolchain-pins/src/lib.rs").read_text()
    expected = re.search(r'EXPECTED_ESBUILD_VERSION: &str = "([^"]+)"', pins)[1]
    require(versions["esbuild"] == expected, "staged esbuild version mismatch")
    require(platform.system() == "Darwin" and platform.machine() == "arm64", "expected native macOS arm64")
    paths = command("git", "ls-files").splitlines()
    paths = [p for p in paths if p.endswith(("Cargo.toml", "Cargo.lock", "package.json")) or p in (
        "pnpm-lock.yaml", "pnpm-workspace.yaml", ".config/nextest.toml",
        "crates/zfb/build.rs", "crates/zfb-toolchain-pins/src/lib.rs",
        ".github/workflows/exam.yml", ".github/actions/native-exam-setup/action.yml",
        "scripts/native-exam.py", "scripts/run-native-exam.sh")]
    hashes = {p: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in sorted(paths)}
    return {"schema": 1, "source": source, "tools": versions, "hashes": hashes,
            "os": command("sw_vers", "-productVersion"), "arch": platform.machine(),
            "rust_verbose": command("rustc", "-vV"),
            "image": os.environ.get("ImageOS"), "image_version": os.environ.get("ImageVersion"),
            "run_id": os.environ["GITHUB_RUN_ID"], "attempt": os.environ["GITHUB_RUN_ATTEMPT"],
            "retries": 0}


def inventory(path, full=False):
    data = read_json(path)
    all_tests, selected, ignored = set(), set(), set()
    for binary, suite in data["rust-suites"].items():
        require(suite["status"] == "listed" and suite["binary-id"] == binary, "unlisted or inconsistent binary")
        for name, test in suite["testcases"].items():
            identity = (binary, name)
            require(type(test["ignored"]) is bool, "invalid ignored flag")
            all_tests.add(identity)
            status = test["filter-match"]["status"]
            require(status in ("matches", "mismatch"), "unknown filter status")
            if test["ignored"]:
                ignored.add(identity)
            if status == "matches":
                selected.add(identity)
    require(len(all_tests) == data["test-count"], "inventory count mismatch")
    if full:
        require(selected == all_tests, "full inventory was filtered")
    else:
        require(not selected & ignored, "selected ignored test")
    return all_tests, selected, ignored


def junit(path):
    root = ET.parse(path).getroot()
    require(root.tag == "testsuites", "unexpected JUnit root")
    for counter in ("failures", "errors", "skipped", "disabled"):
        require(int(root.get(counter, "0")) == 0, f"JUnit total {counter}")
    result = set()
    for suite in root:
        require(suite.tag == "testsuite", "unexpected JUnit root child")
        cases = suite.findall("testcase")
        require(int(suite.attrib["tests"]) == len(cases), "JUnit suite count mismatch")
        for counter in ("failures", "errors", "disabled", "skipped"):
            require(int(suite.get(counter, "0")) == 0, f"JUnit {counter}")
        for case in cases:
            identity = (case.attrib["classname"], case.attrib["name"])
            require(identity[0] == suite.attrib["name"], "JUnit binary identity mismatch")
            require(identity not in result, "duplicate JUnit test")
            require(case.get("status", "run") == "run", "not-run JUnit test")
            duration = float(case.attrib["time"])
            require(math.isfinite(duration) and duration >= 0, "invalid completed test duration")
            require(all(c.tag in ("system-out", "system-err") for c in case), "failed, skipped, retried or unknown JUnit result")
            result.add(identity)
        require(all(c.tag in ("testcase", "properties", "system-out", "system-err") for c in suite), "unknown JUnit suite result")
    require(int(root.attrib["tests"]) == len(result), "JUnit total count mismatch")
    return result


def doctests(list_path, log_path):
    def parse(path, listing):
        crate, identities, outcomes = None, set(), {}
        for line in Path(path).read_text().splitlines():
            match = re.fullmatch(r"\s*Doc-tests (\S+)\s*", line)
            if match:
                crate = match[1]
            match = re.fullmatch(r"(.+): test", line) if listing else re.fullmatch(r"test (.+) \.\.\. (ok|ignored|FAILED)(?:,.*)?", line)
            if match:
                require(crate is not None, "doctest without crate")
                identity = (crate, match[1])
                require(identity not in identities, "duplicate doctest")
                identities.add(identity)
                if not listing:
                    outcomes[identity] = match[2]
        return identities, outcomes
    expected, _ = parse(list_path, True)
    actual, outcomes = parse(log_path, False)
    require(expected and expected == actual, "incomplete doctest inventory/results")
    require("FAILED" not in outcomes.values(), "failed doctest")
    require("ok" in outcomes.values(), "no passing doctests")
    return {"passed": list(outcomes.values()).count("ok"), "ignored": list(outcomes.values()).count("ignored")}


def aggregate(directory, source, run_id, attempt, jobs):
    require(re.fullmatch(r"[0-9a-f]{40}", source), "invalid expected source SHA")
    require(jobs == {"shards": "success", "doctests": "success"}, "failed/cancelled/skipped native job")
    root = Path(directory)
    names = {f"native-exam-shard-{i}" for i in range(1, 5)} | {"native-exam-doctests"}
    require({p.name for p in root.iterdir()} == names, "missing or unexpected native artifact")
    baseline, full_inventory, union = None, None, set()
    timings = {}
    for name in sorted(names):
        folder = root / name
        meta = read_json(folder / "metadata.json")
        require(meta["source"] == source and meta["run_id"] == run_id and meta["attempt"] == attempt, "source/run identity mismatch")
        require(set(meta) == {"schema", "source", "tools", "hashes", "os", "arch", "rust_verbose",
                              "image", "image_version", "run_id", "attempt", "retries"}, "incomplete metadata")
        require(set(meta["tools"]) == {"rust", "cargo", "node", "pnpm", "npm", "nextest", "esbuild"}
                and all(isinstance(v, str) and v for v in meta["tools"].values()), "incomplete tool metadata")
        require(all(isinstance(meta[k], str) and meta[k] for k in ("rust_verbose", "image", "image_version")), "incomplete platform metadata")
        require(meta["schema"] == 1 and meta["retries"] == 0, "invalid evidence schema/retry policy")
        require(meta["arch"] == "arm64" and meta["os"].startswith("15."), "unexpected native platform")
        require(meta["tools"]["rust"].startswith("rustc 1.99.1 ") and meta["tools"]["node"] == "v22.22.2" and meta["tools"]["nextest"].split()[1] == "0.9.148", "unexpected pinned tools")
        require(meta["hashes"] and all(re.fullmatch(r"[0-9a-f]{64}", h) for h in meta["hashes"].values()), "invalid hashes")
        if baseline is None:
            baseline = meta
        require(meta == baseline, "mismatched source/tools/configuration metadata")
        receipt = read_json(folder / "result.json")
        require(receipt["lane"] == name.removeprefix("native-exam-"), "wrong lane receipt")
        require(receipt["completed"] is True and type(receipt["rc"]) is int and receipt["rc"] == 0, "child exit failed or incomplete")
        require((folder / "exit-code.txt").read_text().strip() == "0", "actual child exit failed")
        require((folder / "run.log").stat().st_size > 0, "missing raw log")
        timings[name] = read_json(folder / "timings.json")
        require(set(timings[name]) == {"install_seconds", "build_seconds", "inventory_and_tests_seconds"}
                and all(type(v) is int and v >= 0 for v in timings[name].values()), "invalid timings")
        if name.endswith("doctests"):
            doc_summary = doctests(folder / "full-list.txt", folder / "run.log")
        else:
            full = inventory(folder / "full.json", full=True)
            if full_inventory is None:
                full_inventory = full
            require(full == full_inventory, "full Mac inventories disagree")
            all_tests, selected, ignored = inventory(folder / "selected.json")
            require(all_tests == full[0] and ignored == full[2], "selected inventory differs from full inventory")
            require(selected and not selected & union, "empty or overlapping shard")
            require(junit(folder / "junit.xml") == selected, "missing or unexpected completed test")
            union.update(selected)
    require(union == full_inventory[0] - full_inventory[2], "shard union differs from nonignored Mac inventory")
    return {"verdict": "passed", "source": source, "selected": len(union),
            "ignored": len(full_inventory[2]), "binaries": len({b for b, _ in full_inventory[0]}),
            "doctests": doc_summary, "timings": timings}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("metadata", "aggregate"))
    parser.add_argument("path")
    args = parser.parse_args()
    try:
        source = os.environ["SOURCE_SHA"]
        if args.mode == "metadata":
            result = metadata(source)
            Path(args.path).write_text(json.dumps(result, indent=2) + "\n")
        else:
            result = aggregate(args.path, source, os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"],
                               {"shards": os.environ["SHARDS_RESULT"], "doctests": os.environ["DOCTESTS_RESULT"]})
        print(json.dumps(result, indent=2))
    except (ValueError, KeyError, OSError, ET.ParseError, subprocess.CalledProcessError) as error:
        print(f"Native exam evidence rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
