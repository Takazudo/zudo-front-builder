// A PATH-installed stand-in for `gh` used by the release subprocess suites.
// It never reaches GitHub: Release assets come from FAKE_GH_ASSETS_DIR, the
// `gh release view` JSON from FAKE_GH_RELEASE_JSON, and every invocation is
// appended to FAKE_GH_LOG. Any other command fails loudly (exit 97), so a test
// cannot silently fall through to a mutating call.
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const STUB = String.raw`#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const args = process.argv.slice(2);
if (process.env.FAKE_GH_LOG) fs.appendFileSync(process.env.FAKE_GH_LOG, JSON.stringify(args) + "\n");
const dir = process.env.FAKE_GH_ASSETS_DIR;
const flag = (name) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; };
const all = (name) => args.flatMap((a, i) => (a === name ? [args[i + 1]] : []));
if (args[0] === "release" && args[1] === "view") {
  if (process.env.FAKE_GH_RELEASE_MISSING === "1") { process.stderr.write("release not found\n"); process.exit(1); }
  const jq = flag("--jq");
  if (jq === ".assets[].name") {
    for (const name of fs.readdirSync(dir).sort()) process.stdout.write(name + "\n");
    process.exit(0);
  }
  if (flag("--json") === "assets") {
    const assets = fs.readdirSync(dir).sort().map((name) => ({ name, state: "uploaded", digest: null }));
    process.stdout.write(JSON.stringify({ assets }) + "\n");
    process.exit(0);
  }
  process.stdout.write((process.env.FAKE_GH_RELEASE_JSON || "{}") + "\n");
  process.exit(0);
}
if (args[0] === "release" && args[1] === "download") {
  if (process.env.FAKE_GH_DOWNLOAD_FAIL === "1") { process.stderr.write("download failed\n"); process.exit(1); }
  const out = flag("--dir");
  fs.mkdirSync(out, { recursive: true });
  for (const pattern of all("--pattern")) {
    const source = path.join(dir, pattern);
    if (fs.existsSync(source)) fs.copyFileSync(source, path.join(out, pattern));
  }
  process.exit(0);
}
process.stderr.write("fake gh: unsupported invocation " + JSON.stringify(args) + "\n");
process.exit(97);
`;

/** Create a bin dir holding the stub `gh`; prepend it to PATH. */
export function installFakeGh() {
  const bin = mkdtempSync(join(tmpdir(), "zfb-fake-gh-"));
  const file = join(bin, "gh");
  writeFileSync(file, STUB);
  chmodSync(file, 0o755);
  return bin;
}
