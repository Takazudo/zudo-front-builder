#!/usr/bin/env node

// zfb#2454: fail-closed production artifact metrics. The build log is the
// only place the four overwritten cdylib/bindgen outputs remain observable;
// final wasm/glue and the complete dist are measured from files instead of
// trusting a human-readable summary.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { resolve } from "node:path";

const ARTIFACTS = [
  { label: "default", dir: "wasm", stem: "zfb_md_wasm", ceiling: 1_600_000 },
  {
    label: "highlight-only",
    dir: "wasm-highlight",
    stem: "zfb_md_wasm_highlight",
    ceiling: 820_000,
  },
  { label: "render-only", dir: "wasm-render", stem: "zfb_md_wasm_render", ceiling: 1_100_000 },
  { label: "parse-only", dir: "wasm-parse", stem: "zfb_md_wasm_parse", ceiling: 325_000 },
];
const TARBALL_CEILING = 3_900_000;

function usage() {
  throw new Error(
    "usage: assert-zfb-md-wasm-budgets.mjs --build-log <log> --dist <dist> [--tarball <tgz>]",
  );
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--") || argv[index + 1] === undefined) usage();
    values[arg.slice(2)] = argv[++index];
  }
  if (!values["build-log"] || !values.dist) usage();
  return values;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function bytesFromLine(section, label) {
  // Summary labels are literal (not regex fragments): in particular the
  // `wasm-opt -O1 (final)` parentheses must remain literal.
  const match = section.match(new RegExp(`^${escapeRegExp(label)}:\\s+([0-9,]+) bytes$`, "m"));
  if (!match) throw new Error(`build log is missing ${label} metric`);
  return Number(match[1].replaceAll(",", ""));
}

function parseBuildLog(logPath) {
  return parseBuildLogText(readFileSync(logPath, "utf8"));
}

function parseBuildLogText(log) {
  const sections = new Map();
  for (const artifact of ARTIFACTS) {
    const marker = `-- ${artifact.label} artifact`;
    const start = log.indexOf(marker);
    if (start < 0) throw new Error(`build log is missing ${artifact.label} summary`);
    const next = ARTIFACTS.map(({ label }) =>
      log.indexOf(`-- ${label} artifact`, start + marker.length),
    )
      .filter((offset) => offset >= 0)
      .sort((a, b) => a - b)[0];
    sections.set(artifact.label, log.slice(start, next ?? log.length));
  }
  return sections;
}

function fileBytes(path) {
  const size = statSync(path).size;
  if (!Number.isSafeInteger(size)) throw new Error(`unsafe file size for ${path}`);
  return size;
}

function distBytes(path) {
  const entry = statSync(path);
  if (entry.isFile()) return entry.size;
  if (!entry.isDirectory()) return 0;
  return readdirSync(path).reduce((total, child) => total + distBytes(resolve(path, child)), 0);
}

function fmt(value) {
  return `${value.toLocaleString("en-US")} bytes`;
}

function selfTest() {
  // Fixture mirrors build.mjs's real #2451 summary labels, including commas
  // and literal parentheses in the wasm-opt line.
  const entries = {
    default: ".",
    "highlight-only": "./highlight",
    "render-only": "./render",
    "parse-only": "./parse",
  };
  const fixture = ARTIFACTS.map(
    ({ label }) =>
      `-- ${label} artifact (\`${entries[label]}\` entry) --\n` +
      "raw cdylib:                            1,000 bytes\n" +
      "wasm-bindgen binary:                   900 bytes\n" +
      "generated glue:                        100 bytes\n" +
      "generated glue gzip -9:                50 bytes\n" +
      "wasm-opt -O1 (final):                  80 bytes\n" +
      "gzip -9 (final):                       40 bytes\n",
  ).join("");
  const sections = parseBuildLogText(fixture);
  for (const artifact of ARTIFACTS) {
    const section = sections.get(artifact.label);
    if (bytesFromLine(section, "wasm-bindgen binary") !== 900)
      throw new Error("bindgen fixture parse failed");
    if (bytesFromLine(section, "wasm-opt -O1 (final)") !== 80)
      throw new Error("wasm-opt fixture parse failed");
  }
  console.log("OK: build summary metric parser self-test");
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const sections = parseBuildLog(args["build-log"]);
  const dist = resolve(args.dist);

  if (!statSync(dist).isDirectory()) throw new Error(`dist directory is missing: ${dist}`);

  for (const artifact of ARTIFACTS) {
    const section = sections.get(artifact.label);
    const wasmPath = resolve(dist, artifact.dir, `${artifact.stem}_bg.wasm`);
    const gluePath = resolve(dist, artifact.dir, `${artifact.stem}_glue.zfb-resource.mjs`);
    const wasm = fileBytes(wasmPath);
    const wasmGzip = gzipSync(readFileSync(wasmPath), { level: 9 }).length;
    const glue = fileBytes(gluePath);
    const glueGzip = gzipSync(readFileSync(gluePath), { level: 9 }).length;

    // These two metrics are emitted by the production build before the shared
    // cdylib is overwritten by the next feature pass.
    const cdylib = bytesFromLine(section, "raw cdylib");
    const bindgen = bytesFromLine(section, "wasm-bindgen binary");
    const loggedFinal = bytesFromLine(section, "wasm-opt -O1 (final)");
    if (loggedFinal !== wasm) {
      throw new Error(
        `${artifact.label} final wasm differs from build log: log=${loggedFinal}, file=${wasm}`,
      );
    }
    const loggedGlue = bytesFromLine(section, "generated glue");
    if (loggedGlue !== glue) {
      throw new Error(
        `${artifact.label} glue differs from build log: log=${loggedGlue}, file=${glue}`,
      );
    }

    console.log(
      `${artifact.label}: cdylib=${fmt(cdylib)} bindgen=${fmt(bindgen)} ` +
        `final-wasm=${fmt(wasm)} gzip-9=${fmt(wasmGzip)} ` +
        `glue=${fmt(glue)} glue-gzip-9=${fmt(glueGzip)} ceiling=${fmt(artifact.ceiling)}`,
    );
    if (wasmGzip > artifact.ceiling) {
      throw new Error(
        `${artifact.label} gzip-9 size ${wasmGzip} exceeds ceiling ${artifact.ceiling}`,
      );
    }
  }

  const totalDist = distBytes(dist);
  console.log(`complete dist: ${fmt(totalDist)}`);

  if (args.tarball) {
    const tarball = fileBytes(resolve(args.tarball));
    console.log(`complete packed tarball: ${fmt(tarball)} ceiling=${fmt(TARBALL_CEILING)}`);
    if (tarball > TARBALL_CEILING) {
      throw new Error(`packed tarball size ${tarball} exceeds ceiling ${TARBALL_CEILING}`);
    }
  } else {
    console.log(`complete packed tarball: not supplied (ceiling=${fmt(TARBALL_CEILING)})`);
  }
}

if (process.argv.includes("--self-test")) selfTest();
else main();
