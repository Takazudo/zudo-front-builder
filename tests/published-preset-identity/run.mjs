#!/usr/bin/env node
// Published-preset runtime identity fixture (#3331). See README.md.
//
// Usage: ZFB_BIN=/abs/path/to/zfb node tests/published-preset-identity/run.mjs
//        ZFB_PRESET_BINARY=installed node tests/published-preset-identity/run.mjs
//        (installed mode runs the npm-installed carrier; ZFB_BIN is not used)
//
// Exit 0 only when the positive case passes BOTH oracles and every negative
// control fails the assertions it targets.

import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { createReadStream, createWriteStream, existsSync, statSync } from "node:fs";
import {
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { installedCarrierIdentity, parseBinaryMode } from "./carrier.mjs";
import { consoleOracleFailures, isFatalConsoleMessage } from "./console-policy.mjs";

const fixtureRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(fixtureRoot, "../..");
const siteSource = join(fixtureRoot, "site");
const runTemp = process.env.RUNNER_TEMP || tmpdir();
const evidenceRoot =
  process.env.ZFB_PRESET_EVIDENCE_DIR || join(runTemp, "published-preset-identity");
const SUBPROCESS_DEADLINE_MS = 600_000;
const REGISTRY = "https://registry.npmjs.org/";
const RUNTIME = "@takazudo/zfb";
const DEFINITION_SITE =
  /["']@takazudo\/zfb\/zudo-react\/runtime-definition-v1["']\s*:\s*(?:true|!0)/g;
const PROBE_ROUTE = "/docs/guide/probe";
const SERVER_TEXT = "server:0:unset";
const MOUNT_STEP = "island mounts through the package boundary";
const HYDRATE_STEP = "island hydrates through the package boundary";
const OTHER_ROUTE = "/docs/guide/other";
// Modules the probe Island must pull through the package boundary.
const REQUIRED_SERVER_SOURCES = [
  "src/components/preset-probe.tsx",
  "src/components/preset-probe-boundary.tsx",
  "pages/lib/_chrome-bindings.tsx",
  ".zudo-doc/routes-src/docs-slug.tsx",
  "node_modules/@takazudo/zudo-doc/dist/theme-toggle/index.js",
  "node_modules/@takazudo/zudo-doc/dist/details/details.js",
];

const binaryMode = parseBinaryMode(process.env.ZFB_PRESET_BINARY);

const evidence = {
  binaryMode,
  issue: 3331,
  status: "running",
  node: process.version,
  platform: `${process.platform}/${process.arch}`,
  binary: null,
  pins: null,
  lockfile: null,
  browser: null,
  cases: {},
  failure: null,
};

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const isInside = (parent, child) => {
  const rel = relative(parent, child);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
};
function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function runLogged(command, args, { cwd, logName, env }) {
  const logPath = join(evidenceRoot, logName);
  const log = createWriteStream(logPath, { flags: "w" });
  const child = spawn(command, args, {
    cwd,
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const capture = (chunk) => {
    output += chunk;
    log.write(chunk);
  };
  child.stdout.on("data", capture);
  child.stderr.on("data", capture);
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    child.kill("SIGTERM");
    setTimeout(() => child.kill("SIGKILL"), 5_000).unref();
  }, SUBPROCESS_DEADLINE_MS);
  const code = await new Promise((resolvePromise, rejectPromise) => {
    child.once("error", async (error) => {
      clearTimeout(timer);
      log.write(`spawn error: ${error?.stack ?? error}\n`);
      await new Promise((resolveEnd) => log.end(resolveEnd));
      rejectPromise(error);
    });
    child.once("close", resolvePromise);
  });
  clearTimeout(timer);
  await new Promise((resolvePromise) => log.end(resolvePromise));
  // A liveness expiry is a harness failure; it can never satisfy a negative control.
  if (timedOut)
    throw new Error(`${command} ${args.join(" ")} exceeded ${SUBPROCESS_DEADLINE_MS} ms`);
  return { code, output, log: logPath };
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

// ---------------------------------------------------------------------------
// Inputs: workspace binary, exact pins, frozen lockfile
// ---------------------------------------------------------------------------

async function describeBinary(path, label, extra = {}) {
  const version = await runLogged(path, ["--version"], {
    cwd: repoRoot,
    logName: label === "workspace" ? "zfb-version.log" : `${label}-zfb-version.log`,
  });
  assert(version.code === 0, `${path} --version exited ${version.code}`);
  const sourceSha = await runLogged("git", ["rev-parse", "HEAD"], {
    cwd: repoRoot,
    logName: label === "workspace" ? "checkout-sha.log" : `${label}-checkout-sha.log`,
  });
  return {
    mode: binaryMode,
    path,
    sha256: sha256(await readFile(path)),
    version: version.output.trim(),
    modified: statSync(path).mtime.toISOString(),
    checkoutSha: sourceSha.code === 0 ? sourceSha.output.trim() : null,
    ...extra,
  };
}

async function inspectBinary() {
  const path = process.env.ZFB_BIN;
  assert(
    path && isAbsolute(path),
    "ZFB_BIN must name the absolute path of the workspace-built zfb",
  );
  assert(existsSync(path), `ZFB_BIN does not exist: ${path}`);
  return describeBinary(path, "workspace");
}

// Resolves the zfb binary a case builds with, inside that case's own install root.
async function binaryForCase(installRoot, label) {
  if (binaryMode === "workspace") return evidence.binary.path;
  const carrier = await installedCarrierIdentity(installRoot, siteSource);
  assert(carrier.path && isAbsolute(carrier.path), `unresolved carrier path for ${label}`);
  const npmVersion = await runLogged("npm", ["--version"], {
    cwd: installRoot,
    logName: `${label}-npm-version.log`,
  });
  assert(npmVersion.code === 0, `npm --version exited ${npmVersion.code}`);
  const identity = await describeBinary(carrier.path, label, {
    sha256: carrier.sha256,
    carrierPackage: carrier.packageName,
    carrierVersion: carrier.carrierVersion,
    lockIntegrity: carrier.lockIntegrity,
    nodeVersion: process.version,
    npmVersion: npmVersion.output.trim(),
  });
  (evidence.cases[label] ??= {}).binary = identity;
  return carrier.path;
}

async function verifyPinsAndLockfile() {
  const manifest = await readJson(join(siteSource, "package.json"));
  // Hash the file bytes so the recorded digest equals `sha256sum package-lock.json`.
  const rawLock = await readFile(join(siteSource, "package-lock.json"));
  const lock = JSON.parse(rawLock.toString("utf8"));
  const pins = manifest.dependencies;
  for (const [name, spec] of Object.entries(pins)) {
    assert(/^\d+\.\d+\.\d+$/.test(spec), `${name} must be an exact version, got ${spec}`);
  }
  assert(lock.lockfileVersion === 3, `expected lockfileVersion 3, got ${lock.lockfileVersion}`);
  assert(
    JSON.stringify(lock.packages[""].dependencies) === JSON.stringify(pins),
    "package-lock.json root dependencies drifted from package.json",
  );
  const integrities = {};
  for (const [key, entry] of Object.entries(lock.packages)) {
    if (key === "") continue;
    assert(!entry.link, `${key} is a link; the fixture installs registry tarballs only`);
    assert(entry.resolved?.startsWith(REGISTRY), `${key} does not resolve from ${REGISTRY}`);
    assert(/^sha512-/.test(entry.integrity ?? ""), `${key} has no sha512 integrity`);
  }
  for (const [name, version] of Object.entries(pins)) {
    const entry = lock.packages[`node_modules/${name}`];
    assert(
      entry?.version === version,
      `${name} lock entry is ${entry?.version}, pinned ${version}`,
    );
    integrities[name] = { version, resolved: entry.resolved, integrity: entry.integrity };
  }
  return { pins, integrities, lockSha256: sha256(rawLock) };
}

// ---------------------------------------------------------------------------
// Isolated install outside the repository
// ---------------------------------------------------------------------------

async function prepareInstall(workRoot) {
  const installRoot = join(workRoot, "positive");
  await cp(siteSource, installRoot, {
    recursive: true,
    filter: (source) => !/[/\\](node_modules|dist|\.zfb-build|\.zudo-doc)([/\\]|$)/.test(source),
  });
  const real = await realpath(installRoot);
  assert(
    !isInside(await realpath(repoRoot), real),
    `install root ${real} is inside the repository`,
  );
  // No ancestor node_modules may offer a hoisted fallback for any @takazudo package.
  for (let dir = dirname(real); dir !== dirname(dir); dir = dirname(dir)) {
    assert(
      !existsSync(join(dir, "node_modules", "@takazudo")),
      `ancestor ${dir}/node_modules/@takazudo would allow a hoisted fallback`,
    );
  }
  const install = await runLogged(
    "npm",
    ["ci", "--ignore-scripts", "--no-audit", "--no-fund", `--registry=${REGISTRY}`],
    { cwd: installRoot, logName: "npm-ci.log", env: { npm_config_workspaces: "false" } },
  );
  assert(install.code === 0, `npm ci failed (exit ${install.code}); see ${install.log}`);
  return installRoot;
}

async function findInstalledCopies(nodeModules, name, found = []) {
  let entries;
  try {
    entries = await readdir(nodeModules, { withFileTypes: true });
  } catch {
    return found;
  }
  for (const entry of entries) {
    if (entry.name === ".bin" || entry.name === ".package-lock.json") continue;
    const dirs = entry.name.startsWith("@")
      ? (await readdir(join(nodeModules, entry.name), { withFileTypes: true })).map((child) =>
          join(nodeModules, entry.name, child.name),
        )
      : [join(nodeModules, entry.name)];
    for (const dir of dirs) {
      const manifestPath = join(dir, "package.json");
      if (existsSync(manifestPath) && (await readJson(manifestPath)).name === name) found.push(dir);
      await findInstalledCopies(join(dir, "node_modules"), name, found);
    }
  }
  return found;
}

async function verifyInstalledPackages(installRoot, pins) {
  const installed = {};
  for (const [name, version] of Object.entries(pins)) {
    const dir = join(installRoot, "node_modules", name);
    assert(!(await lstat(dir)).isSymbolicLink(), `${name} is a symlink, not an installed tarball`);
    const actual = (await readJson(join(dir, "package.json"))).version;
    assert(actual === version, `${name} installed ${actual}, pinned ${version}`);
    installed[name] = { version: actual, path: relative(installRoot, await realpath(dir)) };
  }
  return installed;
}

async function runPackageHooks(installRoot, label) {
  // The zudo-sg CLI is part of the published package surface the fixture consumes.
  const bin = join(installRoot, "node_modules", ".bin", "zudo-sg");
  for (const command of ["gen-registry", "gen-token-manifest"]) {
    const run = await runLogged(bin, [command, "--check"], {
      cwd: installRoot,
      logName: `${label}-zudo-sg-${command}.log`,
    });
    assert(run.code === 0, `zudo-sg ${command} --check failed; see ${run.log}`);
  }
}

// Every emitted script under `dir`, including nested chunk directories.
async function listScripts(dir) {
  const files = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await listScripts(path)));
    else if (/\.m?js$/.test(entry.name)) files.push(path);
  }
  return files;
}

// ---------------------------------------------------------------------------
// Oracle 1: installed resolution + emitted runtime identity
// ---------------------------------------------------------------------------

async function identityOracle(installDir) {
  const installRoot = await realpath(installDir);
  const facts = {};
  const failures = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };

  // (a) Installed resolution from the host and from each preset package.
  const resolvers = {
    host: installRoot,
    "@takazudo/zudo-doc": join(installRoot, "node_modules/@takazudo/zudo-doc"),
    "@takazudo/zudo-sg": join(installRoot, "node_modules/@takazudo/zudo-sg"),
  };
  facts.resolution = {};
  for (const [label, from] of Object.entries(resolvers)) {
    const resolved = createRequire(join(from, "package.json")).resolve(`${RUNTIME}/zudo-react`);
    facts.resolution[label] = relative(installRoot, await realpath(resolved));
  }
  const resolved = new Set(Object.values(facts.resolution));
  check(
    resolved.size === 1,
    `installed ${RUNTIME}/zudo-react resolves to ${resolved.size} files: ${[...resolved].join(", ")}`,
  );
  facts.installedCopies = (
    await findInstalledCopies(join(installRoot, "node_modules"), RUNTIME)
  ).map((dir) => relative(installRoot, dir));
  check(
    facts.installedCopies.length === 1,
    `expected one installed ${RUNTIME}, found ${facts.installedCopies.length}: ${facts.installedCopies.join(", ")}`,
  );

  // (b) Server bundle: the sourcemap lists every module esbuild resolved.
  const bundlePath = join(installRoot, ".zfb-build", "bundle.mjs");
  if (!existsSync(bundlePath) || !existsSync(`${bundlePath}.map`)) {
    failures.push("server bundle or its sourcemap is missing");
  } else {
    const map = await readJson(`${bundlePath}.map`);
    // Strip the per-build shadow prefix (`.../zfb-bundler-XXXX/`).
    const sources = map.sources.map((source) => source.replace(/^.*?zfb-bundler-[^/]+\//, ""));
    const runtimeRoots = new Set(
      sources
        .filter((source) => source.includes("/zudo-react/"))
        .map((source) => source.slice(0, source.indexOf("/zudo-react/"))),
    );
    facts.serverRuntimeRoots = [...runtimeRoots];
    check(
      runtimeRoots.size === 1,
      `server bundle resolves zudo-react from ${runtimeRoots.size} roots: ${facts.serverRuntimeRoots.join(", ")}`,
    );
    facts.serverDefinitionSites = [
      ...(await readFile(bundlePath, "utf8")).matchAll(DEFINITION_SITE),
    ].length;
    check(
      facts.serverDefinitionSites === 1,
      `server bundle has ${facts.serverDefinitionSites} runtime definition sites`,
    );
    const missing = REQUIRED_SERVER_SOURCES.filter((wanted) => !sources.includes(wanted));
    check(missing.length === 0, `server bundle lacks probe-path modules: ${missing.join(", ")}`);
    const virtualContents = sources
      .map((source, index) =>
        /\.zfb-virtual-[^/]+\.mjs$/.test(source) ? (map.sourcesContent?.[index] ?? "") : null,
      )
      .filter((content) => content !== null);
    facts.virtualModules = virtualContents.length;
    check(
      virtualContents.some((content) => content.includes('"siteName":"Preset identity fixture"')),
      "virtual:zudo-doc-route-context output for this site is not in the server bundle",
    );
    check(
      virtualContents.some(
        (content) =>
          content.includes("export const sgContext") &&
          content.includes('"categoryOrder":["Actions"]'),
      ),
      "zudo-sg sgContext virtual output for this site is not in the server bundle",
    );
  }

  // (c) Browser assets: every emitted chunk together retains one runtime definition.
  const assets = join(installRoot, "dist", "assets");
  if (!existsSync(assets)) {
    failures.push("client assets are missing");
  } else {
    let sites = 0;
    for (const file of await listScripts(assets)) {
      sites += [...(await readFile(file, "utf8")).matchAll(DEFINITION_SITE)].length;
    }
    facts.clientDefinitionSites = sites;
    check(sites === 1, `client assets retain ${sites} runtime definition sites`);
  }
  return { facts, failures };
}

// ---------------------------------------------------------------------------
// Oracle 2: real Chromium hydration, shared state, navigation, cleanup
// ---------------------------------------------------------------------------

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".xml": "application/xml",
  ".txt": "text/plain; charset=utf-8",
};

async function serveDist(dist) {
  const server = createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url ?? "/", "http://x").pathname);
    const base = resolve(dist, `.${pathname}`);
    const candidates = [base, join(base, "index.html"), `${base}.html`];
    const file = candidates.find(
      (candidate) =>
        isInside(dist, candidate) && existsSync(candidate) && statSync(candidate).isFile(),
    );
    if (!file) {
      response.writeHead(404, { "Content-Type": "text/plain" }).end("Not found");
      return;
    }
    const ext = file.slice(file.lastIndexOf("."));
    response.writeHead(200, { "Content-Type": MIME[ext] ?? "application/octet-stream" });
    createReadStream(file).pipe(response);
  });
  await new Promise((resolvePromise) => server.listen(0, "127.0.0.1", resolvePromise));
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}

async function browserOracle(installRoot, browser) {
  const facts = { console: [], pageErrors: [], steps: [] };
  const failures = [];
  const dist = join(installRoot, "dist");
  if (!existsSync(join(dist, PROBE_ROUTE.slice(1), "index.html"))) {
    return { facts, failures: [`no built ${PROBE_ROUTE} page`] };
  }
  const { server, origin } = await serveDist(dist);
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  context.setDefaultTimeout(10_000);
  const page = await context.newPage();
  page.on("console", (message) => {
    if (isFatalConsoleMessage(message.type())) {
      facts.console.push(`${message.type()}: ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => facts.pageErrors.push(String(error?.stack ?? error)));
  const probe = page.locator("[data-preset-probe]");
  const summary = probe.locator("summary");
  const host = probe.locator("[data-probe-host]");
  const toggle = probe.locator('[data-probe-package-toggle] button[aria-haspopup="menu"]');
  const ledger = () => page.evaluate(() => window.__presetProbe ?? null);
  const step = async (name, action) => {
    try {
      await action();
      facts.steps.push({ name, ok: true });
    } catch (error) {
      facts.steps.push({ name, ok: false, error: String(error?.message ?? error) });
      throw new Error(`${name}: ${error?.message ?? error}`);
    }
  };
  const expectBoth = async (pattern) => {
    await page.waitForFunction(
      (source) => {
        const re = new RegExp(source);
        const root = document.querySelector("[data-preset-probe]");
        const a = root?.querySelector("summary")?.textContent ?? "";
        const b = root?.querySelector("[data-probe-host]")?.textContent ?? "";
        return re.test(a) && a === b;
      },
      pattern.source,
      { timeout: 10_000 },
    );
  };
  try {
    await step("server render precedes activation", async () => {
      const html = await (await fetch(origin + PROBE_ROUTE)).text();
      const matches = html.match(/server:0:unset/g) ?? [];
      assert(
        matches.length === 2,
        `expected server text in package and host portions, saw ${matches.length}`,
      );
      assert(/data-zfb-island="?PresetProbe/.test(html), "PresetProbe island marker missing");
    });
    await step(MOUNT_STEP, async () => {
      await page.goto(origin + PROBE_ROUTE);
      await page
        .locator('[data-zfb-island="PresetProbe"][data-zfb-island-mounted]')
        .waitFor({ timeout: 15_000 });
    });
    await step(HYDRATE_STEP, async () => {
      await expectBoth(/^active:0:(light|dark)$/);
      await page.waitForFunction(
        () => {
          const button = document.querySelector(
            '[data-preset-probe] [data-probe-package-toggle] button[aria-haspopup="menu"]',
          );
          return button && !button.hasAttribute("data-zd-pending");
        },
        null,
        { timeout: 10_000 },
      );
    });
    await step("host signal propagates into the package-rendered portion", async () => {
      // The increment button lives inside the package Details; open it first.
      await summary.click();
      await probe.locator("[data-probe-increment]").click();
      await expectBoth(/^active:1:(light|dark)$/);
    });
    await step("package signals drive the package component and reach the host", async () => {
      await toggle.click();
      await page.getByRole("menuitemradio", { name: "Light", exact: true }).click();
      await page.waitForFunction(
        () =>
          document
            .querySelector(
              '[data-preset-probe] [data-probe-package-toggle] button[aria-haspopup="menu"]',
            )
            ?.getAttribute("aria-label") === "Appearance: Light",
        null,
        { timeout: 10_000 },
      );
      await expectBoth(/^active:1:light$/);
    });
    await step("client navigation away disposes the island", async () => {
      await page.evaluate(() => {
        window.__presetNavigationMarker = "same-document";
      });
      await page.locator(`main a[href="${OTHER_ROUTE}"]`).first().click();
      await page.waitForURL((url) => url.pathname.replace(/\/$/, "") === OTHER_ROUTE, {
        timeout: 10_000,
      });
      await page.waitForFunction(() => !document.querySelector("[data-preset-probe]"), null, {
        timeout: 10_000,
      });
      const marker = await page.evaluate(() => window.__presetNavigationMarker ?? null);
      assert(
        marker === "same-document",
        "navigation reloaded the document instead of using the client router",
      );
      const counts = await ledger();
      assert(
        counts?.activations === 1 && counts?.cleanups === 1,
        `ledger after leaving: ${JSON.stringify(counts)}`,
      );
    });
    await step("history return remounts a live island", async () => {
      await page.goBack();
      await page.waitForURL((url) => url.pathname.replace(/\/$/, "") === PROBE_ROUTE, {
        timeout: 10_000,
      });
      await page
        .locator('[data-zfb-island="PresetProbe"][data-zfb-island-mounted]')
        .waitFor({ timeout: 15_000 });
      await expectBoth(/^active:0:light$/);
      await summary.click();
      await probe.locator("[data-probe-increment]").click();
      await expectBoth(/^active:1:light$/);
      const marker = await page.evaluate(() => window.__presetNavigationMarker ?? null);
      assert(marker === "same-document", "history return reloaded the document");
      const counts = await ledger();
      assert(
        counts?.activations === 2 && counts?.cleanups === 1,
        `ledger after return: ${JSON.stringify(counts)}`,
      );
    });
    failures.push(...consoleOracleFailures(facts));
  } catch (error) {
    failures.push(String(error?.message ?? error));
  } finally {
    facts.finalText = {
      summary: await summary.textContent({ timeout: 1_000 }).catch(() => null),
      host: await host.textContent({ timeout: 1_000 }).catch(() => null),
    };
    facts.ledger = await ledger().catch(() => null);
    await context.close();
    await new Promise((resolvePromise) => server.close(resolvePromise));
  }
  return { facts, failures };
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

async function buildCase(installRoot, label) {
  await rm(join(installRoot, "dist"), { recursive: true, force: true });
  await rm(join(installRoot, ".zfb-build"), { recursive: true, force: true });
  await rm(join(installRoot, ".zudo-doc"), { recursive: true, force: true });
  const zfb = await binaryForCase(installRoot, label);
  const build = await runLogged(zfb, ["build"], {
    cwd: installRoot,
    logName: `${label}-build.log`,
  });
  const routes = [...build.output.matchAll(/package route `([^`]+)`/g)].map((match) => match[1]);
  return {
    exitCode: build.code,
    log: relative(evidenceRoot, build.log),
    packageRoutes: routes,
    output: build.output,
  };
}

async function copyInstall(installRoot, workRoot, label) {
  const target = join(workRoot, label);
  await cp(installRoot, target, {
    recursive: true,
    verbatimSymlinks: true,
    // Only the top-level build outputs; package `dist/` directories must be copied.
    filter: (source) =>
      !/^(dist|\.zfb-build|\.zudo-doc)([/\\]|$)/.test(relative(installRoot, source)),
  });
  return target;
}

async function runPositive(installRoot, browser) {
  const result = { expectation: "pass" };
  evidence.cases.positive = result;
  const build = await buildCase(installRoot, "positive");
  result.build = { exitCode: build.exitCode, log: build.log, packageRoutes: build.packageRoutes };
  assert(build.exitCode === 0, `positive build failed (exit ${build.exitCode}); see ${build.log}`);
  for (const route of ["/docs/[[...slug]]", "/components/[slug]"]) {
    assert(build.packageRoutes.includes(route), `package route ${route} was not injected`);
  }
  assert(
    existsSync(join(installRoot, "dist/components/counter/index.html")),
    "zudo-sg package route did not emit /components/counter",
  );
  result.identity = await identityOracle(installRoot);
  result.browser = await browserOracle(installRoot, browser);
  const failures = [...result.identity.failures, ...result.browser.failures];
  result.outcome = failures.length === 0 ? "passed" : "failed";
  assert(failures.length === 0, `positive case failed:\n  - ${failures.join("\n  - ")}`);
}

// N1: a second owned-runtime copy nested under the published zudo-doc package,
// so zudo-doc's own imports bind to it while the host keeps the canonical one.
async function runNestedCopyNegative(installRoot, workRoot) {
  const result = { expectation: "fail: identity oracle and fail-closed render" };
  evidence.cases["negative-nested-package-copy"] = result;
  const root = await copyInstall(installRoot, workRoot, "negative-nested-package-copy");
  const nested = join(root, "node_modules/@takazudo/zudo-doc/node_modules/@takazudo/zfb");
  await mkdir(dirname(nested), { recursive: true });
  await cp(join(root, "node_modules/@takazudo/zfb"), nested, { recursive: true });
  const build = await buildCase(root, "negative-nested-package-copy");
  // An unrelated earlier ZR_ line must not mask (or be mistaken for) the fail-closed
  // diagnostic, so look for ZR_NO_SCOPE specifically; any other ZR_ line is only
  // reported, and the check below still fails when ZR_NO_SCOPE is absent.
  const diagnostics = build.output.match(/ZR_[A-Z_]+:[^\n]*/g) ?? [];
  const diagnostic =
    diagnostics.find((line) => line.startsWith("ZR_NO_SCOPE:")) ?? diagnostics[0] ?? null;
  result.build = { exitCode: build.exitCode, log: build.log, diagnostic, diagnostics };
  result.identity = await identityOracle(root);
  result.observedFailures = result.identity.failures;
  const facts = result.identity.facts;
  result.checks = {
    "zudo-doc resolves a different zudo-react file":
      facts.resolution["@takazudo/zudo-doc"] !== facts.resolution.host,
    "two installed runtime copies": facts.installedCopies.length === 2,
    "build fails closed with ZR_NO_SCOPE":
      build.exitCode !== 0 &&
      /^ZR_NO_SCOPE: getScope requires synchronous setup/.test(diagnostic ?? ""),
  };
  // The render fails after bundling, so the server bundle is always emitted
  // and must show both runtime copies, while client assets are never emitted.
  result.bundleArtifacts = {
    serverBundle: facts.serverRuntimeRoots !== undefined,
    clientAssets: facts.clientDefinitionSites !== undefined,
  };
  result.checks["server bundle emitted"] = result.bundleArtifacts.serverBundle;
  result.checks["client assets not emitted"] = result.bundleArtifacts.clientAssets === false;
  result.checks["two server runtime roots"] = facts.serverRuntimeRoots?.length === 2;
  result.checks["two server definition sites"] = facts.serverDefinitionSites === 2;
  const missed = Object.entries(result.checks).filter(([, ok]) => !ok);
  assert(
    missed.length === 0,
    `negative-nested-package-copy: did not fail as targeted (${missed
      .map(([name]) => name)
      .join(", ")}); build exit ${build.exitCode} (${diagnostic}), facts ${JSON.stringify(facts)}`,
  );
  result.outcome = "expected-failure";
}

// N2: the host's shared signal comes from a second, separately installed
// runtime copy while the package component and island root use the owned one.
async function runHostCopyNegative(installRoot, workRoot, browser) {
  const result = { expectation: "fail: identity oracle and browser oracle" };
  evidence.cases["negative-host-mismatched-copy"] = result;
  const root = await copyInstall(installRoot, workRoot, "negative-host-mismatched-copy");
  const copy = join(root, "node_modules/@fixture-negative/zfb-copy");
  await mkdir(dirname(copy), { recursive: true });
  await cp(join(root, "node_modules/@takazudo/zfb"), copy, { recursive: true });
  const manifest = await readJson(join(copy, "package.json"));
  manifest.name = "@fixture-negative/zfb-copy";
  await writeFile(join(copy, "package.json"), JSON.stringify(manifest, null, 2));
  const probePath = join(root, "src/components/preset-probe.tsx");
  const original = 'import { computed, getScope, signal } from "@takazudo/zfb/zudo-react";';
  const source = await readFile(probePath, "utf8");
  assert(
    source.includes(original),
    "negative-host-mismatched-copy: probe import line moved; update the harness",
  );
  await writeFile(
    probePath,
    source.replace(
      original,
      'import { computed, getScope } from "@takazudo/zfb/zudo-react";\nimport { signal } from "@fixture-negative/zfb-copy/zudo-react";',
    ),
  );
  const build = await buildCase(root, "negative-host-mismatched-copy");
  result.build = { exitCode: build.exitCode, log: build.log };
  assert(
    build.exitCode === 0,
    `negative-host-mismatched-copy: build must succeed so the oracles run (exit ${build.exitCode})`,
  );
  result.identity = await identityOracle(root);
  result.browser = await browserOracle(root, browser);
  result.observedFailures = [...result.identity.failures, ...result.browser.failures];
  assert(
    result.identity.failures.some((failure) => /runtime definition sites|roots/.test(failure)),
    "negative-host-mismatched-copy: identity oracle did NOT fail",
  );
  // The targeted failure only: the Island mounts, but the host computed cannot
  // track the copy's signal, so hydration never leaves the server text.
  const steps = Object.fromEntries(result.browser.facts.steps.map((entry) => [entry.name, entry]));
  const finalText = result.browser.facts.finalText ?? {};
  result.checks = {
    "island mounted": steps[MOUNT_STEP]?.ok === true,
    "hydration step failed": steps[HYDRATE_STEP]?.ok === false,
    "package portion still server text": finalText.summary === SERVER_TEXT,
    "host portion still server text": finalText.host === SERVER_TEXT,
  };
  const missed = Object.entries(result.checks).filter(([, ok]) => !ok);
  assert(
    missed.length === 0,
    `negative-host-mismatched-copy: browser oracle did not fail as targeted (${missed
      .map(([name]) => name)
      .join(
        ", ",
      )}); steps ${JSON.stringify(result.browser.facts.steps)}, final ${JSON.stringify(finalText)}`,
  );
  result.outcome = "expected-failure";
}

// ---------------------------------------------------------------------------

let browser = null;
let exitCode = 0;
try {
  await rm(evidenceRoot, { recursive: true, force: true });
  await mkdir(evidenceRoot, { recursive: true });
  if (binaryMode === "workspace") evidence.binary = await inspectBinary();
  const pins = await verifyPinsAndLockfile();
  evidence.pins = pins.pins;
  evidence.lockfile = { sha256: pins.lockSha256, pinned: pins.integrities };
  // A missing browser is a failure, never a skip.
  browser = await chromium.launch();
  evidence.browser = { name: "chromium", version: browser.version() };
  const workRoot = await mkdtemp(join(runTemp, "zfb-preset-identity-"));
  evidence.workRoot = workRoot;
  const installRoot = await prepareInstall(workRoot);
  evidence.installed = await verifyInstalledPackages(installRoot, pins.pins);
  await runPackageHooks(installRoot, "positive");
  await runPositive(installRoot, browser);
  await runNestedCopyNegative(installRoot, workRoot);
  await runHostCopyNegative(installRoot, workRoot, browser);
  evidence.status = "passed";
} catch (error) {
  exitCode = 1;
  evidence.status = "failed";
  evidence.failure = String(error?.stack ?? error);
  console.error(`[published-preset-identity] FAILED: ${error?.message ?? error}`);
} finally {
  await browser?.close().catch(() => {});
  // workRoot is scratch (installs and copies); evidence lives in evidenceRoot.
  if (evidence.workRoot && !process.env.ZFB_PRESET_IDENTITY_KEEP) {
    await rm(evidence.workRoot, { recursive: true, force: true }).catch(() => {});
    evidence.workRootRemoved = true;
  }
  await writeFile(join(evidenceRoot, "evidence.json"), JSON.stringify(evidence, null, 2) + "\n");
  const summary = Object.entries(evidence.cases)
    .map(([name, value]) => `${name}=${value.outcome ?? "incomplete"}`)
    .join(" ");
  console.log(`[published-preset-identity] status=${evidence.status} ${summary}`);
  console.log(`[published-preset-identity] evidence: ${join(evidenceRoot, "evidence.json")}`);
}
process.exit(exitCode);
