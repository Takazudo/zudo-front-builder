#!/usr/bin/env node
// Records the selected dependency closure of one producer package for one
// target as a receipt that `--reason cargo-internal-rewire` (#4258) and
// `--reason cargo-removal` (#4388) consume
// (scripts/rebaseline-island-size-{linux,darwin}.mjs).
//
// The closure is what `cargo metadata --locked --filter-platform <target>`
// resolves for the producer: every package reachable over normal and build
// edges (never dev-only edges), with the feature set the workspace-wide
// resolution enables for it (resolver 2 unifies features across all members,
// so this can differ from a `cargo build -p zfb` alone). Lock equality alone
// does not prove equal feature selection, so a rewire
// re-pin requires one receipt at the old lock and one at the new lock, for the
// platform's own target, and refuses any external package, version, source or
// feature movement between them. The receipt binds the Cargo.lock SHA-256 it
// was recorded at, so a receipt from another lock is refused.
//
// Usage:
//   node scripts/record-cargo-producer-closure.mjs --producer zfb \
//     --target x86_64-unknown-linux-gnu --output <file> [--root <checkout>] [--cargo <cargo>]
//
// `--target` may name a target the host cannot compile for (`cargo metadata`
// resolves without a toolchain for it), so the Darwin arm64 receipt can be
// recorded on Linux from the same checkout. Rows are sorted by bytes and path
// sources are recorded relative to the checkout root, so two checkouts of the
// same tree produce identical `rows`; `cargo` and `command` record the machine.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
// Compare canonical paths: a symlinked invocation path (macOS /var -> /private/var) must still run main() (#4465).
import { realpathSync as realpathSyncForMain } from "node:fs";

export const CLOSURE_RECEIPT_SCHEMA = 1;
export const CLOSURE_EDGES = "normal,build";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

// `path+file:///abs/crates/zfb#0.0.0` -> `path+crates/zfb` relative to `root`;
// every other source (registry, git) is kept verbatim without its `#version`
// suffix, which the row's `version` already carries.
export function normalizeSource(id, root) {
  const hash = id.lastIndexOf("#");
  const source = hash < 0 ? id : id.slice(0, hash);
  if (!source.startsWith("path+file://")) return source;
  const path = fileURLToPath(source.slice("path+".length));
  const rel = relative(resolve(root), path).split(sep).join("/");
  if (rel === "") return "path+.";
  if (rel.startsWith("..") || isAbsolute(rel)) {
    throw new Error("path dependency " + path + " lies outside the checkout root " + root);
  }
  return "path+" + rel;
}

// The producer's closure rows from a parsed `cargo metadata` document: walks
// `resolve.nodes` from the producer over edges whose dep_kinds include a
// normal (null) or build kind, and records {name, version, source, features}
// per reached package, sorted by name, version and source.
export function closureFromMetadata(metadata, { producer, root }) {
  const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const roots = metadata.packages.filter(
    (pkg) => pkg.name === producer && metadata.workspace_members.includes(pkg.id),
  );
  if (roots.length !== 1) {
    throw new Error(
      "producer " + producer + " must be exactly one workspace member, found " + roots.length,
    );
  }
  const reached = new Set();
  const queue = [roots[0].id];
  while (queue.length > 0) {
    const id = queue.pop();
    if (reached.has(id)) continue;
    reached.add(id);
    const node = nodes.get(id);
    if (!node) throw new Error("resolve graph lacks " + id);
    for (const dep of node.deps) {
      const kinds = dep.dep_kinds ?? [];
      const selected = kinds.some((kind) => kind.kind === null || kind.kind === "build");
      if (selected) queue.push(dep.pkg);
    }
  }
  const rows = [...reached].map((id) => {
    const pkg = packages.get(id);
    const node = nodes.get(id);
    if (!pkg) throw new Error("metadata lacks package " + id);
    return {
      name: pkg.name,
      version: pkg.version,
      source: normalizeSource(id, root),
      features: [...new Set(node.features)].sort(),
    };
  });
  const byBytes = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  rows.sort(
    (a, b) =>
      byBytes(a.name, b.name) || byBytes(a.version, b.version) || byBytes(a.source, b.source),
  );
  return rows;
}

export function parseArgs(argv) {
  const values = { cargo: "cargo", root: "." };
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined) {
      throw new Error("bad argument: " + argv[i]);
    }
    values[argv[i].slice(2)] = argv[i + 1];
  }
  for (const key of ["producer", "target", "output"]) {
    if (!values[key]) throw new Error("required --" + key);
  }
  for (const key of Object.keys(values)) {
    if (!["producer", "target", "output", "root", "cargo"].includes(key)) {
      throw new Error("unknown argument --" + key);
    }
  }
  return values;
}

export function metadataCommand(target) {
  return ["metadata", "--locked", "--format-version", "1", "--filter-platform", target];
}

export function recordClosure({ producer, target, root, cargo = "cargo" }) {
  const args = metadataCommand(target);
  const metadata = JSON.parse(
    execFileSync(cargo, args, { cwd: root, maxBuffer: 256 * 1024 * 1024 }).toString("utf8"),
  );
  const cargoVersion = execFileSync(cargo, ["--version"], { cwd: root }).toString("utf8").trim();
  return {
    schemaVersion: CLOSURE_RECEIPT_SCHEMA,
    producer,
    target,
    edges: CLOSURE_EDGES,
    cargoLockSha256: sha256(readFileSync(resolve(root, "Cargo.lock"))),
    command: [cargo, ...args],
    cargo: cargoVersion,
    rows: closureFromMetadata(metadata, { producer, root }),
  };
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  const receipt = recordClosure(args);
  writeFileSync(args.output, JSON.stringify(receipt, null, 2) + "\n");
  console.log(
    "recorded " +
      receipt.rows.length +
      " closure rows of " +
      receipt.producer +
      " for " +
      receipt.target +
      " at Cargo.lock " +
      receipt.cargoLockSha256 +
      " -> " +
      args.output,
  );
}

const invokedPath = process.argv[1] && realpathSyncForMain(process.argv[1]);
if (invokedPath === realpathSyncForMain(fileURLToPath(import.meta.url))) {
  try {
    main();
  } catch (error) {
    console.error("record-cargo-producer-closure: " + error.message);
    process.exit(1);
  }
}
