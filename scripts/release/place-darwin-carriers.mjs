#!/usr/bin/env node
// place-darwin-carriers.mjs — copy admitted owner-Mac Darwin bytes into the two
// platform packages, re-verifying every digest against the admission verdict
// immediately before placing (#4690). The Darwin carriers get their bytes only
// from here; no hosted macOS build exists.
//
//   node scripts/release/place-darwin-carriers.mjs --verdict verdict.json \
//        --inputs DIR [--packages-root DIR]
//
// Exit 0 placed, 1 a digest/shape mismatch (nothing placed), 2 usage error.
import { createHash } from "node:crypto";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseTarGz } from "../admit-darwin-artifacts.mjs";
import { VERDICT_SCHEMA, assetNames } from "../darwin-artifact-contract.mjs";
import { isMain } from "./gh.mjs";

export const CARRIER_DIRS = Object.freeze({
  "aarch64-apple-darwin": "packages/zfb-darwin-arm64",
  "x86_64-apple-darwin": "packages/zfb-darwin-x64",
});

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Pure-ish: verify and return the bytes to place, or throw listing every problem. */
export function preparePlacement({ verdict, inputsDir }) {
  const problems = [];
  if (verdict?.schema !== VERDICT_SCHEMA) problems.push(`verdict schema is ${verdict?.schema}`);
  if (verdict?.admitted !== true) problems.push("verdict is not admitted");
  const placements = [];
  for (const [triple, dir] of Object.entries(CARRIER_DIRS)) {
    const target = verdict?.targets?.[triple];
    const names = assetNames(verdict?.version, triple);
    if (!target) {
      problems.push(`${triple}: absent from the verdict`);
      continue;
    }
    const read = (name) => {
      const path = join(inputsDir, name);
      if (!existsSync(path)) {
        problems.push(`${triple}: ${name} missing from the admitted inputs`);
        return null;
      }
      return readFileSync(path);
    };
    const archive = read(names.archive);
    const addon = read(names.addon);
    if (!archive || !addon) continue;
    if (target.archive?.name !== names.archive || sha256(archive) !== target.archive?.sha256) {
      problems.push(`${triple}: ${names.archive} differs from the admitted digest`);
      continue;
    }
    if (target.addon?.name !== names.addon || sha256(addon) !== target.addon?.sha256) {
      problems.push(`${triple}: ${names.addon} differs from the admitted digest`);
      continue;
    }
    let entries;
    try {
      entries = parseTarGz(archive);
    } catch (error) {
      problems.push(`${triple}: archive unreadable: ${error.message}`);
      continue;
    }
    if (entries.length !== 1 || entries[0].name !== "zfb") {
      problems.push(`${triple}: archive is not exactly one root "zfb"`);
      continue;
    }
    const binary = Buffer.from(entries[0].data);
    if (sha256(binary) !== target.binary?.sha256) {
      problems.push(`${triple}: extracted zfb differs from the admitted binary digest`);
      continue;
    }
    placements.push({ triple, dir, binary, addon });
  }
  if (problems.length) {
    const error = new Error(problems.join("; "));
    error.problems = problems;
    throw error;
  }
  return placements;
}

export function place({ verdict, inputsDir, packagesRoot }) {
  const placements = preparePlacement({ verdict, inputsDir });
  for (const { triple, dir, binary, addon } of placements) {
    const root = join(packagesRoot, dir);
    if (!existsSync(join(root, "package.json"))) throw new Error(`${dir}/package.json not found`);
    writeFileSync(join(root, "zfb"), binary);
    chmodSync(join(root, "zfb"), 0o755);
    writeFileSync(join(root, "zfb-css.node"), addon);
    process.stdout.write(
      `placed admitted ${triple} zfb (${sha256(binary)}) + zfb-css.node into ${dir}\n`,
    );
  }
  return placements;
}

export function main(argv) {
  const opts = { packagesRoot: "." };
  for (let i = 0; i < argv.length; i += 2) {
    const value = argv[i + 1];
    if (!value) return usage();
    if (argv[i] === "--verdict") opts.verdict = value;
    else if (argv[i] === "--inputs") opts.inputsDir = value;
    else if (argv[i] === "--packages-root") opts.packagesRoot = value;
    else return usage();
  }
  if (!opts.verdict || !opts.inputsDir) return usage();
  let verdict;
  try {
    verdict = JSON.parse(readFileSync(opts.verdict, "utf8"));
  } catch (error) {
    process.stderr.write(`place-darwin-carriers: cannot read verdict: ${error.message}\n`);
    return 2;
  }
  try {
    place({ verdict, inputsDir: opts.inputsDir, packagesRoot: opts.packagesRoot });
  } catch (error) {
    process.stdout.write(`::error::Darwin carrier placement refused: ${error.message}\n`);
    return 1;
  }
  return 0;
}

function usage() {
  process.stderr.write(
    "usage: place-darwin-carriers.mjs --verdict FILE --inputs DIR [--packages-root DIR]\n",
  );
  return 2;
}

if (isMain(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
