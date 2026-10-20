#!/usr/bin/env node
// Derives a neutral local Darwin receipt (a NEW identity) from a private
// original without touching it: listed absolute prefixes in `commands` and
// `bundle.location` become placeholders, and `operator` may be replaced.
// #4314 reconciliation plan, item 0 (owner decisions D1 yes, D3 no).
// Prints only placeholders, counts and digests, never a replaced value.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, hostname, userInfo } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs as parseNodeArgs } from "node:util";
import { pathToFileURL } from "node:url";

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const PLACEHOLDER = /^<[A-Z][A-Z0-9_]*>$/;
// Fixed fragments that must never survive into a publishable receipt.
const FORBIDDEN_FRAGMENTS = ["/Users/", "/private/", "/Volumes/", "/home/", "/root/"];
const FIXED_KEYS = ["schemaVersion", "kind", "collectedAt", "source", "platform"];

function fail(message) {
  throw new Error("neutralize-local-receipt: " + message);
}

export function readMap(map) {
  if (!map || typeof map !== "object" || Array.isArray(map)) fail("map must be an object");
  const keys = Object.keys(map).sort();
  for (const key of keys)
    if (!["forbidden", "operator", "prefixes"].includes(key)) fail("unknown map key " + key);
  if (!Array.isArray(map.prefixes) || map.prefixes.length === 0) fail("prefixes are required");
  for (const entry of map.prefixes) {
    if (
      !entry ||
      typeof entry.from !== "string" ||
      !entry.from.startsWith("/") ||
      entry.from.length < 2 ||
      typeof entry.to !== "string" ||
      !PLACEHOLDER.test(entry.to)
    )
      fail("each prefix needs an absolute `from` and a `<PLACEHOLDER>` `to`");
  }
  if (map.operator !== undefined && (typeof map.operator !== "string" || !map.operator.trim()))
    fail("operator, when given, must be non-empty text");
  if (
    map.forbidden !== undefined &&
    (!Array.isArray(map.forbidden) ||
      !map.forbidden.every((value) => typeof value === "string" && value.length > 0))
  )
    fail("forbidden must be a list of non-empty strings");
  // Longest first, so a nested prefix never splits a longer one.
  return {
    prefixes: [...map.prefixes].sort((a, b) => b.from.length - a.from.length),
    operator: map.operator,
    forbidden: map.forbidden ?? [],
  };
}

function neutralText(text, prefixes, counts) {
  let out = text;
  for (const { from, to } of prefixes) {
    const parts = out.split(from);
    if (parts.length > 1) counts[to] = (counts[to] ?? 0) + parts.length - 1;
    out = parts.join(to);
  }
  return out;
}

function localIdentityFragments() {
  const values = [];
  for (const read of [() => homedir(), () => userInfo().username, () => hostname()]) {
    try {
      const value = read();
      // Very short values (e.g. a one-letter user) would match ordinary text.
      if (typeof value === "string" && value.length >= 3) values.push(value);
    } catch {
      // Unavailable identity sources are simply not scanned.
    }
  }
  return values;
}

export function neutralizeReceipt({
  originalBytes,
  expectedSha,
  sourceSha,
  bundleDigest,
  map,
  evidence,
  identityFragments = localIdentityFragments(),
}) {
  if (sha(originalBytes) !== expectedSha) fail("original receipt digest mismatch");
  const original = JSON.parse(originalBytes.toString("utf8"));
  evidence.validateLocalReceipt(original, sourceSha);
  if (original.bundle.sha256 !== bundleDigest) fail("retained bundle digest mismatch");
  const { prefixes, operator, forbidden } = readMap(map);
  const counts = {};
  const neutral = structuredClone(original);
  neutral.commands = original.commands.map((argv) =>
    argv.map((arg) => neutralText(arg, prefixes, counts)),
  );
  neutral.bundle = {
    location: neutralText(original.bundle.location, prefixes, counts),
    sha256: original.bundle.sha256,
  };
  if (operator !== undefined) neutral.operator = operator;

  if (JSON.stringify(Object.keys(neutral)) !== JSON.stringify(Object.keys(original)))
    fail("field set changed");
  for (const key of FIXED_KEYS)
    if (JSON.stringify(neutral[key]) !== JSON.stringify(original[key])) fail(key + " changed");
  const changed = {
    commands: neutral.commands.flat().filter((arg, i) => arg !== original.commands.flat()[i])
      .length,
    bundleLocation: neutral.bundle.location !== original.bundle.location,
    operator: neutral.operator !== original.operator,
  };
  const bytes = Buffer.from(JSON.stringify(neutral, null, 2) + "\n");
  const text = bytes.toString("utf8");
  const leaks = [
    ...FORBIDDEN_FRAGMENTS,
    ...prefixes.map(({ from }) => from),
    ...forbidden,
    ...identityFragments,
    ...(operator !== undefined ? [original.operator] : []),
  ].filter((fragment) => text.includes(fragment));
  if (leaks.length > 0)
    fail(leaks.length + " forbidden fragment(s) remain in the neutral receipt; nothing written");
  evidence.validateLocalReceipt(neutral, sourceSha);
  return {
    bytes,
    report: {
      originalSha256: expectedSha,
      neutralSha256: sha(bytes),
      bundleSha256: neutral.bundle.sha256,
      sourceSha,
      placeholders: Object.fromEntries(Object.entries(counts).sort()),
      changedCommandArgs: changed.commands,
      bundleLocationReplaced: changed.bundleLocation,
      operatorReplaced: changed.operator,
    },
  };
}

export async function main(argv = process.argv.slice(2)) {
  const { values } = parseNodeArgs({
    args: argv,
    options: {
      tools: { type: "string" },
      receipt: { type: "string" },
      "expected-sha": { type: "string" },
      "source-sha": { type: "string" },
      bundle: { type: "string" },
      map: { type: "string" },
      out: { type: "string" },
    },
    strict: true,
  });
  for (const key of ["tools", "receipt", "expected-sha", "source-sha", "bundle", "map", "out"])
    if (!values[key]) fail("required --" + key);
  const out = resolve(values.out);
  if (existsSync(out)) fail("--out already exists; never overwrite");
  const evidence = await import(
    pathToFileURL(join(resolve(values.tools), "scripts/island-size-local-evidence.mjs")).href
  );
  const receiptPath = resolve(values.receipt);
  const originalBytes = readFileSync(receiptPath);
  const { bytes, report } = neutralizeReceipt({
    originalBytes,
    expectedSha: values["expected-sha"],
    sourceSha: values["source-sha"],
    bundleDigest: evidence.localBundleDigest(values.bundle),
    map: JSON.parse(readFileSync(values.map, "utf8")),
    evidence,
  });
  writeFileSync(out, bytes, { flag: "wx" });
  if (sha(readFileSync(receiptPath)) !== values["expected-sha"])
    fail("original receipt changed during the run");
  if (sha(readFileSync(out)) !== report.neutralSha256) fail("written neutral receipt differs");
  console.log(JSON.stringify(report, null, 2));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
