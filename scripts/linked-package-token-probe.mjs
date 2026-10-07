#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { chmod, copyFile, lstat, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { chromium } from "@playwright/test";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const runTemp = process.env.RUNNER_TEMP || tmpdir();
const evidenceRoot =
  process.env.ZFB_PROBE_EVIDENCE_DIR || join(runTemp, "linked-package-token-probe");
const scratchRoot = await mkdtemp(join(runTemp, "zfb-3931-linked-probe-"));
const appRoot = join(scratchRoot, "app");
const widgetRoot = join(scratchRoot, "widget");
const binaryPath = resolve(repoRoot, "target/debug/zfb");
const url = "http://127.0.0.1:44991/";
const fixtureFiles = {
  ".gitignore": "node_modules/\n.zfb-build/\ndist/\ntest-results/\n",
  "package.json":
    JSON.stringify({ name: "zfb-linked-package-probe", private: true, version: "1.0.0" }, null, 2) +
    "\n",
  "pnpm-workspace.yaml": [
    "packages:",
    "  - app",
    "  - widget",
    "linkWorkspacePackages: false",
    "minimumReleaseAgeExclude:",
    "  - '@takazudo/zfb-darwin-arm64@4.0.0'",
    "  - '@takazudo/zfb-darwin-x64@4.0.0'",
    "  - '@takazudo/zfb-linux-arm64-gnu@4.0.0'",
    "  - '@takazudo/zfb-linux-x64-gnu@4.0.0'",
    "  - '@takazudo/zfb-runtime@4.0.0'",
    "  - '@takazudo/zfb-win32-x64-msvc@4.0.0'",
    "  - '@takazudo/zfb@4.0.0'",
    "",
  ].join("\n"),
  "app/package.json":
    JSON.stringify(
      {
        name: "watch-repro-app",
        type: "module",
        private: true,
        dependencies: {
          "@takazudo/zfb": "4.0.0",
          "@takazudo/zfb-runtime": "4.0.0",
          "@sample/widget": "workspace:*",
        },
      },
      null,
      2,
    ) + "\n",
  "app/zfb.config.ts":
    "import {defineConfig} from '@takazudo/zfb/config';\n" +
    "export default defineConfig({wind:false,plugins:[{name:'./plugins/alias.mjs'}]});\n",
  "app/plugins/alias.mjs":
    "export default {name:'widget-alias',setup(ctx){ctx.addAlias('@sample/widget','../widget/dist/index.js');}};\n",
  "app/pages/index.tsx":
    "import {Island} from '@takazudo/zfb';\n" +
    "import {Probe} from '../components/probe';\n" +
    'export default function Page(){return <html><head><title>Linked identity</title></head><body><Island when="load"><Probe /></Island></body></html>;}\n',
  "app/components/probe.tsx":
    "'use client';\n" +
    "import {signal} from '@takazudo/zfb/zudo-react';\n" +
    "import {label} from '@sample/widget';\n" +
    "export function Probe(){\n" +
    "  const count=signal(0);\n" +
    '  return <button type="button" on:click={() => { count.value += 1; }}>{label}: {count}</button>;\n' +
    "}\n",
  "widget/package.json":
    JSON.stringify(
      {
        name: "@sample/widget",
        type: "module",
        version: "1.0.0",
        exports: "./dist/index.js",
        files: ["dist"],
      },
      null,
      2,
    ) + "\n",
  "widget/dist/index.js": "export const label = 'Widget';\n",
};

const result = {
  issue: 3931,
  sourceIssue: 3901,
  status: "running",
  checkoutSha: null,
  binary: { path: binaryPath, sha256: null, version: null },
  pnpmVersion: null,
  node: process.version,
  platform: process.platform + "/" + process.arch,
  runtimePackage: "@takazudo/zfb-runtime@4.0.0 from npm",
  cliPackage: "@takazudo/zfb@4.0.0 from npm; app/node_modules/.bin/zfb wraps the branch binary",
  scratchRoot,
  port: 44991,
  watcherCaveat:
    "This probe checks token identity. Ignored writes may still trigger full SSR ticks, tracked by #3926.",
  installExitCode: null,
  fixtureFiles: [],
  phases: { quiet: [], ignored_json: [], ignored_trace: [], source_change: [] },
  hydration: { status: "not-run", identityErrors: [], console: [], pageErrors: [], clicked: false },
  acceptance: {},
  failure: null,
};

let devProcess = null;
let devLog = null;
let browser = null;
let exitCode = 0;

const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const saveJson = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + "\n");
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const isInside = (parent, child) => {
  const rel = relative(parent, child);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
};

async function saveFixtureFile(relPath, contents) {
  const bytes = Buffer.from(contents);
  const target = join(scratchRoot, relPath);
  const evidenceCopy = join(evidenceRoot, "fixture", relPath);
  await mkdir(dirname(target), { recursive: true });
  await mkdir(dirname(evidenceCopy), { recursive: true });
  await writeFile(target, bytes);
  await writeFile(evidenceCopy, bytes);
  result.fixtureFiles.push({ path: relPath, bytes: bytes.byteLength, sha256: sha256(bytes) });
}

async function runLogged(command, args, cwd, logPath) {
  const log = createWriteStream(logPath, { flags: "w" });
  const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
  child.stdout.on("data", (chunk) => {
    process.stdout.write(chunk);
    log.write(chunk);
  });
  child.stderr.on("data", (chunk) => {
    process.stderr.write(chunk);
    log.write(chunk);
  });
  const code = await new Promise((resolvePromise, rejectPromise) => {
    child.once("error", rejectPromise);
    child.once("close", resolvePromise);
  });
  await new Promise((resolvePromise) => log.end(resolvePromise));
  return code;
}

async function getIdentity(phase, index) {
  const response = await fetch(url, { signal: AbortSignal.timeout(15000) });
  const html = await response.text();
  assert(response.ok, phase + " returned HTTP " + response.status);
  const ids = [...html.matchAll(/data-zfb-build="([^"]+)"/g)].map((match) => match[1]);
  const unique = [...new Set(ids)];
  assert(ids.length > 0, phase + " response has no data-zfb-build");
  assert(
    unique.length === 1,
    phase + " response has multiple identities: " + JSON.stringify(unique),
  );
  await writeFile(join(evidenceRoot, "latest.html"), html);
  return { phase, index, identity: unique[0], ids, at: new Date().toISOString() };
}

async function waitForServer() {
  let lastError = "no response";
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (devProcess.exitCode !== null) throw new Error("zfb dev exited before serving the fixture");
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(5000) });
      if (response.ok) return;
      lastError = "HTTP " + response.status;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await delay(500);
  }
  throw new Error("timed out waiting for dev server: " + lastError);
}

async function checkHydration(expectedIdentity) {
  const pageResult = {
    status: "running",
    expectedIdentity,
    browserVersion: null,
    identityErrors: [],
    console: [],
    pageErrors: [],
    beforeClick: null,
    afterClick: null,
    clicked: false,
  };
  let page = null;
  try {
    browser = await chromium.launch({ headless: true });
    pageResult.browserVersion = browser.version();
    page = await browser.newPage();
    page.on("console", (message) => {
      const entry = { type: message.type(), text: message.text() };
      pageResult.console.push(entry);
      if (entry.text.includes("ZR_IDENTITY")) pageResult.identityErrors.push(entry.text);
    });
    page.on("pageerror", (error) => {
      const entry = error.stack || error.message;
      pageResult.pageErrors.push(entry);
      if (entry.includes("ZR_IDENTITY")) pageResult.identityErrors.push(entry);
    });

    const response = await page.goto(url, { waitUntil: "load", timeout: 60000 });
    assert(response && response.ok(), "Chromium page load failed");
    const island = page.locator('[data-zfb-island="Probe"]');
    const button = island.locator("button");
    await button.waitFor({ state: "visible", timeout: 30000 });
    assert(
      (await island.getAttribute("data-zfb-build")) === expectedIdentity,
      "browser page identity differs from ignored_json result",
    );
    await page.waitForTimeout(1000);
    pageResult.beforeClick = (await button.textContent())?.replace(/\s+/g, " ").trim();
    assert(
      /^Widget:\s*0$/.test(pageResult.beforeClick || ""),
      "expected initial button text Widget: 0",
    );
    await button.click();
    await page.waitForFunction(
      () => {
        const target = document.querySelector('[data-zfb-island="Probe"] button');
        return /^Widget:\s*1$/.test((target?.textContent || "").replace(/\s+/g, " ").trim());
      },
      undefined,
      { timeout: 15000 },
    );
    pageResult.afterClick = (await button.textContent())?.replace(/\s+/g, " ").trim();
    pageResult.clicked = true;
    pageResult.status = "passed";
  } catch (error) {
    pageResult.status = "failed";
    pageResult.error = error instanceof Error ? error.stack || error.message : String(error);
  } finally {
    if (page) await page.close().catch(() => {});
    pageResult.identityErrors = [...new Set(pageResult.identityErrors)];
    result.hydration = pageResult;
    await saveJson(join(evidenceRoot, "browser-result.json"), pageResult);
    await saveJson(join(evidenceRoot, "results.json"), result);
  }
}

async function captureFinalFixture() {
  const finalFiles = [];
  for (const relPath of [
    "widget/dist/index.js",
    "widget/test-results/probe.json",
    "widget/test-results/probe.trace",
  ]) {
    try {
      const bytes = await readFile(join(scratchRoot, relPath));
      const copy = join(evidenceRoot, "fixture-final", relPath);
      await mkdir(dirname(copy), { recursive: true });
      await writeFile(copy, bytes);
      finalFiles.push({ path: relPath, bytes: bytes.byteLength, sha256: sha256(bytes) });
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  return finalFiles;
}

async function stopDevServer() {
  if (devProcess?.exitCode === null && devProcess.pid) {
    try {
      process.kill(-devProcess.pid, "SIGTERM");
    } catch (error) {
      if (error?.code !== "ESRCH") throw error;
    }
    await Promise.race([
      new Promise((resolvePromise) => devProcess.once("close", resolvePromise)),
      delay(5000),
    ]);
    if (devProcess.exitCode === null) {
      try {
        process.kill(-devProcess.pid, "SIGKILL");
      } catch (error) {
        if (error?.code !== "ESRCH") throw error;
      }
    }
  }
  if (devLog) await new Promise((resolvePromise) => devLog.end(resolvePromise));
}

async function main() {
  await mkdir(evidenceRoot, { recursive: true });
  assert(!isInside(repoRoot, scratchRoot), "scratch fixture must be outside the repository");
  result.checkoutSha = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
  const binary = await readFile(binaryPath);
  result.binary.sha256 = sha256(binary);
  result.binary.version = execFileSync(binaryPath, ["--version"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
  result.pnpmVersion = execFileSync("pnpm", ["--version"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();

  for (const [relPath, contents] of Object.entries(fixtureFiles)) {
    await saveFixtureFile(relPath, contents);
  }
  result.installExitCode = await runLogged(
    "pnpm",
    ["install"],
    scratchRoot,
    join(evidenceRoot, "pnpm-install.log"),
  );
  assert(result.installExitCode === 0, "pnpm install failed; see pnpm-install.log");

  try {
    await copyFile(
      join(scratchRoot, "pnpm-lock.yaml"),
      join(evidenceRoot, "fixture", "pnpm-lock.yaml"),
    );
    const lock = await readFile(join(scratchRoot, "pnpm-lock.yaml"));
    result.fixtureFiles.push({
      path: "pnpm-lock.yaml",
      bytes: lock.byteLength,
      sha256: sha256(lock),
      generated: true,
    });
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }

  const shimPath = join(appRoot, "node_modules/.bin/zfb");
  try {
    await lstat(shimPath);
    await rm(shimPath, { force: true });
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const shim =
    "#!/usr/bin/env bash\nset -euo pipefail\nexec " + JSON.stringify(binaryPath) + ' "$@"\n';
  await writeFile(shimPath, shim);
  await chmod(shimPath, 0o755);
  await writeFile(join(evidenceRoot, "zfb-wrapper.sh"), shim);
  await saveJson(join(evidenceRoot, "results.json"), result);

  devLog = createWriteStream(join(evidenceRoot, "dev.log"), { flags: "w" });
  devProcess = spawn(
    "pnpm",
    ["exec", "zfb", "dev", "--scratch-dir", ".zfb-build/dev", "--port", "44991"],
    { cwd: appRoot, env: process.env, detached: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  devProcess.stdout.on("data", (chunk) => devLog.write(chunk));
  devProcess.stderr.on("data", (chunk) => devLog.write(chunk));
  await new Promise((resolvePromise, rejectPromise) => {
    devProcess.once("spawn", resolvePromise);
    devProcess.once("error", rejectPromise);
  });
  await waitForServer();

  for (const phase of ["quiet", "ignored_json", "ignored_trace", "source_change"]) {
    for (let index = 0; index < 4; index += 1) {
      if (phase === "ignored_json") {
        await mkdir(join(widgetRoot, "test-results"), { recursive: true });
        await writeFile(join(widgetRoot, "test-results/probe.json"), JSON.stringify({ i: index }));
      } else if (phase === "ignored_trace") {
        await mkdir(join(widgetRoot, "test-results"), { recursive: true });
        await writeFile(
          join(widgetRoot, "test-results/probe.trace"),
          JSON.stringify({ i: index, event: "snapshot" }),
        );
      } else if (phase === "source_change" && index === 0) {
        await writeFile(
          join(widgetRoot, "dist/index.js"),
          "export const label = 'Widget changed 3931';\n",
        );
      }
      await delay(phase === "source_change" && index === 0 ? 5000 : 2000);
      result.phases[phase].push(await getIdentity(phase, index));
      await saveJson(join(evidenceRoot, "results.json"), result);
    }
    if (phase === "ignored_json") {
      await checkHydration(result.phases.ignored_json[3]?.identity || "");
    }
  }

  const ids = Object.fromEntries(
    Object.entries(result.phases).map(([phase, samples]) => [
      phase,
      samples.map((sample) => sample.identity),
    ]),
  );
  const baseline = ids.quiet[0];
  result.acceptance = {
    fourRequestsPerPhase: Object.values(ids).every((values) => values.length === 4),
    quietStable: ids.quiet.length === 4 && ids.quiet.every((id) => id === baseline),
    ignoredJsonStable:
      ids.ignored_json.length === 4 && ids.ignored_json.every((id) => id === baseline),
    ignoredTraceStable:
      ids.ignored_trace.length === 4 && ids.ignored_trace.every((id) => id === baseline),
    sourceChangeNew:
      ids.source_change.length === 4 && ids.source_change.every((id) => id !== baseline),
    sourceChangeStable:
      ids.source_change.length === 4 &&
      ids.source_change.every((id) => id === ids.source_change[0]),
    hydrationClicked: result.hydration.status === "passed" && result.hydration.clicked,
    noBrowserIdentityError: result.hydration.identityErrors.length === 0,
    identities: ids,
  };
  await saveJson(join(evidenceRoot, "identity-results.json"), result.acceptance);
  const failed = Object.entries(result.acceptance)
    .filter(([key, value]) => key !== "identities" && value !== true)
    .map(([key]) => key);
  assert(failed.length === 0, "probe failed: " + failed.join(", "));
}

try {
  await mkdir(evidenceRoot, { recursive: true });
  await saveJson(join(evidenceRoot, "results.json"), result);
  await main();
  result.status = "passed";
} catch (error) {
  exitCode = 1;
  result.status = "failed";
  result.failure = error instanceof Error ? error.stack || error.message : String(error);
  console.error(result.failure);
} finally {
  try {
    if (browser) await browser.close();
    await stopDevServer();
  } catch (error) {
    exitCode = 1;
    result.status = "failed";
    result.failure =
      (result.failure ? result.failure + "\n" : "") +
      "cleanup: " +
      (error instanceof Error ? error.stack || error.message : String(error));
  }
  try {
    const finalFiles = await captureFinalFixture();
    await mkdir(evidenceRoot, { recursive: true });
    await saveJson(join(evidenceRoot, "results.json"), result);
    await saveJson(join(evidenceRoot, "fixture-manifest.json"), {
      issue: 3901,
      root: scratchRoot,
      packageVersions: {
        "@takazudo/zfb": "4.0.0 from npm",
        "@takazudo/zfb-runtime": "4.0.0 from npm",
        "@sample/widget": "workspace package",
      },
      branchBinary: result.binary,
      appBinWrapper: "app/node_modules/.bin/zfb wraps the branch-built target/debug/zfb",
      watcherCaveat: result.watcherCaveat,
      sourceFiles: result.fixtureFiles,
      finalStateFiles: finalFiles,
    });
    await writeFile(
      join(evidenceRoot, "fixture-file-list.txt"),
      result.fixtureFiles
        .map((entry) => entry.path)
        .concat(finalFiles.map((entry) => "fixture-final/" + entry.path))
        .join("\n") + "\n",
    );
    if (!isInside(repoRoot, scratchRoot)) await rm(scratchRoot, { recursive: true, force: true });
  } catch (error) {
    exitCode = 1;
    console.error("could not finalize probe evidence: " + String(error));
  }
}

if (exitCode !== 0) process.exitCode = exitCode;
