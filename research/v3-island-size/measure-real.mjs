#!/usr/bin/env node
// Runs the real zfb CLI against a temporary consumer, sequentially.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, normalize, relative, resolve } from "node:path";
import { gzipSync } from "node:zlib";

const root = dirname(new URL(import.meta.url).pathname);
const fixtures = join(root, "fixtures");
const cases = {
  "no-island": [],
  "event-only": ["event-only"],
  "scalar-signal": ["scalar-signal"],
  "show-for": ["show-for"],
  model: ["model"],
  "blog-theme": ["blog-menu", "theme-toggle"],
  "json-api": ["json-api"],
  "multi-island": ["scalar-signal", "theme-toggle"],
};
const names = {
  "event-only": "EventOnly",
  "scalar-signal": "ScalarSignal",
  "show-for": "ShowFor",
  model: "Model",
  "blog-menu": "BlogMenu",
  "theme-toggle": "ThemeToggle",
  "json-api": "JsonApi",
};
function option(flag) {
  const i = process.argv.indexOf(flag);
  if (i < 0 || !process.argv[i + 1]) throw new Error(`required ${flag} VALUE`);
  return process.argv[i + 1];
}
const zfb = resolve(option("--zfb"));
const esbuild = resolve(option("--esbuild"));
const packageDir = resolve(option("--package"));
const out = resolve(option("--out"));
const mode = option("--mode");
const sourceSha = option("--source-sha");
const caseIndex = process.argv.indexOf("--case");
const onlyCase = caseIndex < 0 ? null : process.argv[caseIndex + 1];
if (onlyCase !== null && !Object.hasOwn(cases, onlyCase))
  throw new Error(`unknown --case ${onlyCase}; use ${Object.keys(cases).join(", ")}`);
if (!["workspace", "packed"].includes(mode)) throw new Error("--mode must be workspace or packed");
if (!/^[0-9a-f]{40}$/.test(sourceSha)) throw new Error("--source-sha must be a full SHA");
const version = execFileSync(esbuild, ["--version"], { encoding: "utf8" }).trim();
if (version !== "0.25.12") throw new Error(`expected esbuild 0.25.12, got ${version}`);
const packageJson = JSON.parse(readFileSync(join(packageDir, "package.json"), "utf8"));
if (packageJson.name !== "@takazudo/zfb") throw new Error("wrong package path");
if (mode === "packed") readFileSync(join(packageDir, "dist", "zudo-react", "client.js"));
function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
function walk(dir) {
  const rows = [];
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, item.name);
    if (item.isDirectory()) rows.push(...walk(path));
    else if (item.isFile()) rows.push(path);
  }
  return rows.sort();
}
function projectPage(ids) {
  let source = 'import { Island } from "@takazudo/zfb";\n';
  for (const id of ids) source += `import { ${names[id]} } from "../components/${id}";\n`;
  source +=
    'export default function Home(){return <html lang="en"><head><title>Island size</title></head><body><main>Fixture</main>';
  for (const id of ids) source += `<Island when="load"><${names[id]} /></Island>`;
  return source + "</body></html>;}\n";
}
function build(label, ids, pass) {
  const work = mkdtempSync(join(tmpdir(), "zfb-real-island-size-"));
  try {
    mkdirSync(join(work, "pages"));
    mkdirSync(join(work, "components"));
    mkdirSync(join(work, "node_modules", "@takazudo"), { recursive: true });
    symlinkSync(packageDir, join(work, "node_modules", "@takazudo", "zfb"), "dir");
    writeFileSync(join(work, "zfb.config.json"), '{"wind":false}\n');
    writeFileSync(join(work, "pages", "index.tsx"), projectPage(ids));
    for (const id of ids)
      copyFileSync(join(fixtures, `${id}.tsx`), join(work, "components", `${id}.tsx`));
    try {
      execFileSync(zfb, ["build"], {
        cwd: work,
        env: { ...process.env, ZFB_ESBUILD_BIN: esbuild },
        stdio: "pipe",
      });
    } catch (error) {
      throw new Error(
        `${label} pass ${pass}: zfb build failed\n${error.stdout?.toString() ?? ""}\n${error.stderr?.toString() ?? ""}`,
      );
    }
    const dist = join(work, "dist");
    const files = walk(dist).map((path) => ({
      path: relative(dist, path),
      sha256: sha(readFileSync(path)),
    }));
    const jsPaths = walk(dist).filter((path) => /\.(?:js|mjs|cjs)$/.test(path));
    const html = readFileSync(join(dist, "index.html"), "utf8");
    const emitted = new Map(jsPaths.map((path) => [relative(dist, path), path]));
    const initial = new Set();
    const htmlScripts = [...html.matchAll(/(?:src|href)=["']([^"']+\.(?:js|mjs|cjs))["']/g)].map(
      (x) => x[1],
    );
    const entryPaths = [...emitted.keys()].filter(
      (x) => /^assets\/islands(?:-[^/]+)?\.js$/.test(x) && !x.startsWith("assets/islands-chunk-"),
    );
    if (ids.length && entryPaths.length !== 1)
      throw new Error(`${label}: expected one island entry, found ${entryPaths}`);
    if (!ids.length && entryPaths.length)
      throw new Error("no-island unexpectedly shipped islands runtime");
    if (ids.length && !html.includes(`/assets/${basename(entryPaths[0])}`))
      throw new Error(`${label}: entry absent from HTML`);
    function visit(path) {
      if (initial.has(path)) return;
      const file = emitted.get(path);
      if (!file) throw new Error(`missing imported chunk ${path}`);
      initial.add(path);
      const code = readFileSync(file, "utf8");
      const staticImports = [
        /\bimport\s*(?!\()(?:[^;'"()]*?\bfrom\s*)?["'](\.\/[^"']+\.(?:js|mjs|cjs))["']/g,
        /\bexport\s+[^;'"()]*?\bfrom\s*["'](\.\/[^"']+\.(?:js|mjs|cjs))["']/g,
      ];
      for (const pattern of staticImports)
        for (const match of code.matchAll(pattern)) visit(resolveImport(path, match[1]));
    }
    for (const path of entryPaths) visit(path);
    for (const src of htmlScripts) {
      const path = src.replace(/^\//, "");
      if (emitted.has(path)) visit(path);
    }
    const dest = join(out, label, `pass-${pass}`);
    mkdirSync(dest, { recursive: true });
    copyFileSync(join(dist, "index.html"), join(dest, "index.html"));
    const chunks = jsPaths.map((path) => {
      const file = relative(dist, path);
      const bytes = readFileSync(path);
      const target = join(dest, file);
      mkdirSync(dirname(target), { recursive: true });
      copyFileSync(path, target);
      return {
        file,
        phase: initial.has(file) ? "initial" : "later",
        role: entryPaths.includes(file) ? "entry" : initial.has(file) ? "shared" : "lazy",
        raw: bytes.length,
        gzip: gzipSync(bytes, { level: 9, mtime: 0 }).length,
        sha256: sha(bytes),
      };
    });
    const sum = (rows) => ({
      raw: rows.reduce((n, x) => n + x.raw, 0),
      gzip: rows.reduce((n, x) => n + x.gzip, 0),
    });
    return {
      files,
      chunks,
      entryPaths,
      initial: sum(chunks.filter((x) => x.phase === "initial")),
      later: sum(chunks.filter((x) => x.phase === "later")),
      shipped: sum(chunks),
    };
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}
function resolveImport(from, specifier) {
  return normalize(join(dirname(from), specifier));
}

mkdirSync(out, { recursive: true });
const results = {};
for (const [label, ids] of Object.entries(cases).filter(
  ([name]) => !onlyCase || name === onlyCase,
)) {
  const one = build(label, ids, 1);
  const two = build(label, ids, 2);
  if (JSON.stringify(one) !== JSON.stringify(two))
    throw new Error(`${label}: repeat builds differ; inspect ${join(out, label)}`);
  results[label] = one;
}
const report = {
  provenance: {
    sourceSha,
    mode,
    packageVersion: packageJson.version,
    packageJsonSha256: sha(readFileSync(join(packageDir, "package.json"))),
    packagePath: packageDir,
    zfbBinarySha256: sha(readFileSync(zfb)),
    esbuildVersion: version,
    esbuildBinarySha256: sha(readFileSync(esbuild)),
    nodeVersion: process.version,
    zlibVersion: process.versions.zlib,
    compression: "node:zlib gzipSync level=9 mtime=0 per JS file",
    repeats: 2,
  },
  results,
  incremental:
    results["multi-island"] && results["scalar-signal"]
      ? {
          secondIslandVersusScalar: {
            raw: results["multi-island"].shipped.raw - results["scalar-signal"].shipped.raw,
            gzip: results["multi-island"].shipped.gzip - results["scalar-signal"].shipped.gzip,
          },
        }
      : null,
};
writeFileSync(join(out, "measurement.json"), JSON.stringify(report, null, 2) + "\n");
let markdown = `# Real zfb build: ${mode}\n\nSource ${sourceSha}; package ${packageJson.version}; esbuild ${version}; Node ${process.version}; zlib ${process.versions.zlib}; gzipSync level 9, mtime 0. Two byte-identical dist inventories per case: **PASS**.\n\n| Case | Initial raw | Initial gzip | Later raw | Later gzip | All shipped raw | All shipped gzip |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n`;
for (const [label, value] of Object.entries(results))
  markdown += `| ${label} | ${value.initial.raw} | ${value.initial.gzip} | ${value.later.raw} | ${value.later.gzip} | ${value.shipped.raw} | ${value.shipped.gzip} |\n`;
markdown +=
  "\n| Case | Chunk | Phase | Role | Raw | Gzip | SHA-256 |\n| --- | --- | --- | --- | ---: | ---: | --- |\n";
for (const [label, value] of Object.entries(results))
  for (const chunk of value.chunks)
    markdown += `| ${label} | ${chunk.file} | ${chunk.phase} | ${chunk.role} | ${chunk.raw} | ${chunk.gzip} | ${chunk.sha256} |\n`;
if (report.incremental)
  markdown += `\nMulti-island incremental against scalar signal: ${report.incremental.secondIslandVersusScalar.raw} raw, ${report.incremental.secondIslandVersusScalar.gzip} gzip.\n`;
markdown += `\nEvery emitted JS file is counted once. Sidecar metafiles in the matching modeled report attribute retained inputs; zfb's private resource and stage-audit metafiles are untouched. Historical v2/v3 recipe numbers are not reproduced.\n`;
writeFileSync(join(out, "report.md"), markdown);
console.log(join(out, "report.md"));
