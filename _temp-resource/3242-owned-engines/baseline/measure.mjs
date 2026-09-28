#!/usr/bin/env node

/**
 * Reproducible release-build baseline collector for issue #3245.
 *
 * Usage:
 *   node measure.mjs <repo-root> <release-zfb-binary> <output-dir> [sample-count]
 *
 * All scaffold copies and build output stay in output-dir, which must be
 * outside repo-root. Each basic-blog sample gets a fresh copy for its cold
 * build and then reuses that copy for its warm build. The first sample's
 * built site is left in place for the separate browser capture script.
 */

import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const [repoArg, zfbArg, outputArg, samplesArg = "5"] = process.argv.slice(2);

if (!repoArg || !zfbArg || !outputArg) {
  console.error(
    "usage: node measure.mjs <repo-root> <release-zfb-binary> <output-dir> [sample-count]",
  );
  process.exit(2);
}

const repoRoot = path.resolve(repoArg);
const zfbBinary = path.resolve(zfbArg);
const outputRoot = path.resolve(outputArg);
const sampleCount = Number.parseInt(samplesArg, 10);

if (!Number.isInteger(sampleCount) || sampleCount < 1) {
  throw new Error(`sample-count must be a positive integer; received ${samplesArg}`);
}
if (!fs.statSync(repoRoot).isDirectory()) {
  throw new Error(`repo-root is not a directory: ${repoRoot}`);
}
if (!fs.statSync(zfbBinary).isFile()) {
  throw new Error(`release zfb binary does not exist: ${zfbBinary}`);
}
if (Object.hasOwn(process.env, "ZFB_EMBEDDED_NODE_MODULES_CACHE")) {
  throw new Error(
    "ZFB_EMBEDDED_NODE_MODULES_CACHE must be unset for this measurement; unset it and rerun.",
  );
}

const relativeOutput = path.relative(repoRoot, outputRoot);
if (relativeOutput === "" || (!relativeOutput.startsWith("..") && !path.isAbsolute(relativeOutput))) {
  throw new Error(`output-dir must be outside repo-root: ${outputRoot}`);
}

fs.mkdirSync(outputRoot, { recursive: true });
if (fs.readdirSync(outputRoot).length > 0) {
  throw new Error(`output-dir must be empty: ${outputRoot}`);
}

const basicBlogTemplate = path.join(repoRoot, "crates/zfb/templates/basic-blog");
const smokeFixture = path.join(repoRoot, "tests/built-site-smoke/fixture-site");
const cachelessEnv = { ...process.env };
delete cachelessEnv.ZFB_EMBEDDED_NODE_MODULES_CACHE;

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function run(command, args = [], options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with status ${result.status}:\n${result.stderr ?? result.stdout}`,
    );
  }
  return (result.stdout ?? "").trim();
}

function tryRun(command, args = [], options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (result.error || result.status !== 0) return null;
  return (result.stdout ?? "").trim();
}

function git(args) {
  return run("git", ["-C", repoRoot, ...args]);
}

function commandVersion(command, args = ["--version"]) {
  return tryRun(command, args) ?? "unavailable";
}

function hostMetadata() {
  const platform = os.platform();
  const host = {
    hostname: os.hostname(),
    platform,
    release: os.release(),
    architecture: os.arch(),
    cpuModel: os.cpus()[0]?.model ?? "unknown",
    logicalCpuCount: os.cpus().length,
    memoryBytes: os.totalmem(),
  };

  if (platform === "darwin") {
    host.osVersion = tryRun("sw_vers", ["-productVersion"]) ?? os.release();
    host.cpuModel = tryRun("sysctl", ["-n", "machdep.cpu.brand_string"]) ?? host.cpuModel;
    host.logicalCpuCount = Number(tryRun("sysctl", ["-n", "hw.logicalcpu"]) ?? host.logicalCpuCount);
    host.memoryBytes = Number(tryRun("sysctl", ["-n", "hw.memsize"]) ?? host.memoryBytes);
  } else if (platform === "linux") {
    const distro = tryRun("bash", ["-lc", "sed -n 's/^PRETTY_NAME=//p' /etc/os-release"]);
    if (distro) host.osVersion = distro.replace(/^"|"$/g, "");
    const cpu = tryRun("bash", ["-lc", "lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -1"]);
    if (cpu) host.cpuModel = cpu;
    const memory = tryRun("bash", ["-lc", "awk '/MemTotal:/ {print $2 * 1024}' /proc/meminfo"]);
    if (memory) host.memoryBytes = Number(memory);
  } else {
    host.osVersion = os.version?.() ?? os.release();
  }

  return host;
}

function collectZfbEnvironment() {
  const result = {};
  for (const key of Object.keys(cachelessEnv).sort()) {
    if (key.startsWith("ZFB_")) result[key] = cachelessEnv[key];
  }
  result.ZFB_EMBEDDED_NODE_MODULES_CACHE = "unset (default)";
  return result;
}

function treeFiles(root) {
  const files = [];
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isSymbolicLink()) {
        const link = fs.readlinkSync(full);
        files.push({ path: path.relative(root, full), bytes: 0, symlink: link });
      } else if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile()) {
        files.push({ path: path.relative(root, full), bytes: fs.statSync(full).size });
      }
    }
  }
  walk(root);
  return files;
}

function fileTreeSummary(root) {
  const files = treeFiles(root);
  return {
    fileCount: files.filter((file) => !file.symlink).length,
    totalBytes: files.reduce((sum, file) => sum + file.bytes, 0),
    files,
  };
}

function copyFixture(source, destination) {
  const excludedNames = new Set(["node_modules", "dist", ".zfb-build", ".zfb"]);
  fs.cpSync(source, destination, {
    recursive: true,
    filter(sourcePath) {
      const relative = path.relative(source, sourcePath);
      if (!relative) return true;
      return !relative.split(path.sep).some((segment) => excludedNames.has(segment));
    },
  });
}

function timingLines(stderr) {
  return stderr.split(/\r?\n/).filter((line) => line.includes("[zfb-build-timing]"));
}

function runBuild(siteDir, label, timingEnabled) {
  const env = { ...cachelessEnv };
  if (timingEnabled) env.ZFB_BUILD_TIMING = "1";
  const start = performance.now();
  const result = spawnSync(zfbBinary, ["build"], {
    cwd: siteDir,
    env,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const elapsedMs = Number((performance.now() - start).toFixed(3));
  if (result.error) throw result.error;

  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  fs.writeFileSync(path.join(outputRoot, `${label}.stdout.log`), stdout);
  fs.writeFileSync(path.join(outputRoot, `${label}.stderr.log`), stderr);
  if (result.status !== 0) {
    throw new Error(
      `build failed (${label}) with status ${result.status}; see ${label}.stdout.log and ${label}.stderr.log in ${outputRoot}`,
    );
  }

  return {
    label,
    cwd: siteDir,
    command: `cd ${shellQuote(siteDir)} && ${timingEnabled ? "ZFB_BUILD_TIMING=1 " : ""}${shellQuote(zfbBinary)} build`,
    wallMs: elapsedMs,
    timingLines: timingLines(stderr),
    stdoutLog: `${label}.stdout.log`,
    stderrLog: `${label}.stderr.log`,
  };
}

function outputSummary(siteDir) {
  const distDir = path.join(siteDir, "dist");
  const summary = fileTreeSummary(distDir);
  const normalizedFiles = summary.files.map((file) => ({
    ...file,
    path: file.path.split(path.sep).join("/"),
  }));
  const htmlFiles = normalizedFiles.filter((file) => file.path.endsWith(".html"));
  const cssFiles = normalizedFiles.filter((file) => file.path.startsWith("assets/") && file.path.endsWith(".css"));
  const javascriptAssets = normalizedFiles.filter((file) => file.path.startsWith("assets/") && file.path.endsWith(".js"));

  return {
    fileCount: summary.fileCount,
    totalBytes: summary.totalBytes,
    htmlPages: htmlFiles.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    cssAssets: cssFiles.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    javascriptAssets: javascriptAssets.map(({ path: filePath, bytes }) => ({ path: filePath, bytes })),
    files: normalizedFiles,
  };
}

function sumPathBytes(root) {
  const stat = fs.lstatSync(root);
  if (stat.isSymbolicLink()) return 0;
  if (stat.isFile()) return stat.size;
  if (!stat.isDirectory()) return 0;
  return fs.readdirSync(root).reduce((sum, name) => sum + sumPathBytes(path.join(root, name)), 0);
}

function releaseVendorEntries(releaseBuildDir) {
  const found = tryRun("find", [
    releaseBuildDir,
    "-maxdepth",
    "3",
    "-type",
    "d",
    "-name",
    "vendor",
    "-path",
    "*zfb-*",
  ]);
  if (!found) return { matches: [], selected: null, entries: [] };
  const matches = found.split(/\r?\n/).filter(Boolean);
  const selected = matches
    .map((directory) => ({ directory, mtimeMs: fs.statSync(directory).mtimeMs }))
    .sort((a, b) => b.mtimeMs - a.mtimeMs)[0]?.directory;
  if (!selected) return { matches, selected: null, entries: [] };
  const entries = fs.readdirSync(selected).sort().map((name) => {
    const full = path.join(selected, name);
    return { name, bytes: sumPathBytes(full), isDirectory: fs.lstatSync(full).isDirectory() };
  });
  return {
    matches,
    selected,
    selectionRule: matches.length > 1 ? "most recently modified vendor directory" : "only match",
    entries,
    esbuildBytes: fs.existsSync(path.join(selected, "bin/esbuild")) ? fs.statSync(path.join(selected, "bin/esbuild")).size : null,
    tailwindBytes: fs.existsSync(path.join(selected, "bin/tailwindcss-v4")) ? fs.statSync(path.join(selected, "bin/tailwindcss-v4")).size : null,
  };
}

function stagedBinaries(binaryDir) {
  if (!fs.existsSync(binaryDir)) return [];
  return treeFiles(binaryDir)
    .filter((file) => {
      const basename = path.basename(file.path);
      return basename !== ".gitkeep" && basename !== "README.md" && !file.symlink;
    })
    .map((file) => ({ path: file.path.split(path.sep).join("/"), bytes: file.bytes }));
}

const tailwindMarkerDir = os.tmpdir();
const tailwindMarkers = fs.existsSync(tailwindMarkerDir)
  ? fs.readdirSync(tailwindMarkerDir).filter((name) => /^zfb-tailwind-oxide-warmup-.*\.done$/.test(name)).sort()
  : [];

const sampleResults = [];
let basicBlogOutput = null;
for (let index = 1; index <= sampleCount; index += 1) {
  const siteDir = path.join(outputRoot, `basic-blog-${String(index).padStart(2, "0")}`);
  copyFixture(basicBlogTemplate, siteDir);
  const cold = runBuild(siteDir, `basic-blog-${String(index).padStart(2, "0")}-cold`, true);
  const warm = runBuild(siteDir, `basic-blog-${String(index).padStart(2, "0")}-warm`, true);
  sampleResults.push({ index, siteDir, cold, warm });
  if (index === 1) basicBlogOutput = outputSummary(siteDir);
}

const smokeSiteDir = path.join(outputRoot, "built-site-smoke");
copyFixture(smokeFixture, smokeSiteDir);
const smokeBuild = runBuild(smokeSiteDir, "built-site-smoke", false);
const smokeOutput = outputSummary(smokeSiteDir);

const releaseDir = path.join(repoRoot, "target/release");
const releaseBinaryStat = fs.statSync(zfbBinary);
const vendor = releaseVendorEntries(path.join(releaseDir, "build"));
const staged = stagedBinaries(path.join(repoRoot, "crates/zfb/binaries"));
const mergeBase = git(["merge-base", "HEAD", "origin/main"]);
const preChangeDiffStat = tryRun("git", [
  "-C",
  repoRoot,
  "diff",
  "--stat",
  mergeBase,
  "HEAD",
  "--",
  "crates",
  "packages",
  "tests",
]);

const results = {
  metadata: {
    revision: git(["rev-parse", "HEAD"]),
    mergeBaseWithOriginMain: mergeBase,
    preChangeDiffStat: preChangeDiffStat || "(empty)",
    host: hostMetadata(),
    tools: {
      rustc: commandVersion("rustc"),
      node: commandVersion("node"),
      pnpm: commandVersion("pnpm"),
      zfb: commandVersion(zfbBinary),
    },
    zfbEnvironment: collectZfbEnvironment(),
    tailwindWarmupMarkersBeforeFirstColdSample: tailwindMarkers,
    tailwindWarmupMarkerDirectory: tailwindMarkerDir,
    zfbBinary: { path: zfbBinary, bytes: releaseBinaryStat.size },
    embeddedVendor: vendor,
    stagedBinaries: staged,
    helper: path.relative(repoRoot, path.join(scriptDir, "measure.mjs")),
  },
  sampleCount,
  basicBlog: {
    siteForVisualCapture: sampleResults[0]?.siteDir,
    samples: sampleResults,
    outputFromFirstSampleAfterWarmBuild: basicBlogOutput,
  },
  builtSiteSmoke: {
    siteDir: smokeSiteDir,
    build: smokeBuild,
    output: smokeOutput,
  },
};

fs.writeFileSync(path.join(outputRoot, "measurements.json"), `${JSON.stringify(results, null, 2)}\n`);

const rawLines = (key) => {
  const samples = sampleResults.map((sample) => sample[key]);
  return samples.map((sample) => `### ${sample.label}\nwall_ms=${sample.wallMs}\n${sample.timingLines.join("\n")}`).join("\n\n");
};
const timingStats = (key) => {
  const values = sampleResults.map((sample) => sample[key].wallMs).sort((a, b) => a - b);
  const median = values.length % 2
    ? values[(values.length - 1) / 2]
    : (values[values.length / 2 - 1] + values[values.length / 2]) / 2;
  return { minMs: values[0], medianMs: Number(median.toFixed(3)), maxMs: values.at(-1) };
};

const report = [
  `# Measurement output for issue #3245`,
  ``,
  `Output directory: ${outputRoot}`,
  `Revision: ${results.metadata.revision}`,
  `Host: ${JSON.stringify(results.metadata.host)}`,
  `Tools: ${JSON.stringify(results.metadata.tools)}`,
  `ZFB environment: ${JSON.stringify(results.metadata.zfbEnvironment)}`,
  `Tailwind warm-up markers before first cold sample: ${JSON.stringify(tailwindMarkers)}`,
  `Pre-change diff stat: ${results.metadata.preChangeDiffStat}`,
  ``,
  `Release binary bytes: ${results.metadata.zfbBinary.bytes}`,
  `Vendor directory selected: ${vendor.selected ?? "not found"}`,
  `Vendor matches: ${JSON.stringify(vendor.matches)}`,
  `Vendor entries: ${JSON.stringify(vendor.entries)}`,
  `Staged binary entries: ${JSON.stringify(staged)}`,
  `zfb --version: ${results.metadata.tools.zfb}`,
  ``,
  `Basic-blog cold wall time min/median/max (ms): ${JSON.stringify(timingStats("cold"))}`,
  `Basic-blog warm wall time min/median/max (ms): ${JSON.stringify(timingStats("warm"))}`,
  ``,
  `## Raw cold timing lines`,
  ``,
  "```text",
  rawLines("cold"),
  "```",
  ``,
  `## Raw warm timing lines`,
  ``,
  "```text",
  rawLines("warm"),
  "```",
  ``,
  `## Emitted output summaries`,
  ``,
  `Basic-blog: ${JSON.stringify(basicBlogOutput)}`,
  ``,
  `Built-site-smoke: ${JSON.stringify(smokeOutput)}`,
  ``,
  `Basic-blog capture site: ${sampleResults[0]?.siteDir}`,
  `Built-site-smoke build command: ${smokeBuild.command}`,
  ``,
].join("\n");
fs.writeFileSync(path.join(outputRoot, "measurements.md"), report);

console.log(`Measurements written to ${path.join(outputRoot, "measurements.md")}`);
console.log(`Structured data written to ${path.join(outputRoot, "measurements.json")}`);
console.log(`Basic-blog visual-capture site: ${sampleResults[0]?.siteDir}`);
