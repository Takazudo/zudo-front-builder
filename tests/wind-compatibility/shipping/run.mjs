#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { chromium } from "@playwright/test";
import {
  browserIdentity,
  requiredMatrixMember,
} from "../../../scripts/wind-compatibility/browser-adapter.mjs";
import {
  loadReference,
  treeDigest,
} from "../../../scripts/wind-compatibility/differential-runner.mjs";
import { verifyProductionBuild } from "../../../scripts/wind-compatibility/production-build.mjs";
import { testedInputIdentity } from "../../../scripts/wind-compatibility/reference-identity.mjs";
import {
  validateShippingEvidence,
  shippingManifestPath,
} from "../../../scripts/wind-compatibility/reference-shipping.mjs";
import {
  digest,
  fromRoot,
  outsideCheckout,
  readJson,
  sha256,
} from "../../../scripts/wind-compatibility/reference.mjs";
import { verifyDistProof } from "../../wind-real-build/dist-proof.mjs";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, value, index, all) => {
    if (index % 2 === 0) {
      if (!value.startsWith("--") || !all[index + 1]) throw Error("Expected --key value pairs");
      pairs.push([value.slice(2), all[index + 1]]);
    }
    return pairs;
  }, []),
);
for (const key of [
  "production-build",
  "comparison",
  "plan",
  "dist",
  "dist-proof",
  "reference-cache",
  "output",
])
  if (!args[key]) throw Error(`Missing --${key}`);

const output = await outsideCheckout(args.output);
await mkdir(output, { recursive: true });
const comparison = await readJson(args.comparison);
const plan = await readJson(args.plan);
const manifest = await readJson(fromRoot(shippingManifestPath));
const profile = await readJson(fromRoot("tests/wind-compatibility/profile.json"));
const inputs = await testedInputIdentity();
if (comparison.testedInputs.digest !== inputs.digest)
  throw Error("Comparison was not run against the current shipping inputs");
const productionBuild = await readJson(args["production-build"]);
await verifyProductionBuild(productionBuild, comparison);
const proof = await readJson(args["dist-proof"]);
await verifyDistProof(proof, args.dist);
if (digest(proof.productionBuild) !== digest(productionBuild))
  throw Error("Dist proof uses a different production binary");
const reference = await loadReference(args["reference-cache"], comparison.candidate.reference);
if (digest(reference.identity) !== digest(comparison.candidate.reference))
  throw Error("SRI-verified reference identity differs from comparison");

const browserExecutable = chromium.executablePath();
const browser = await chromium.launch({ headless: true, executablePath: browserExecutable });
const browserEnvironment = await browserIdentity(browser, browserExecutable);
browserEnvironment.styleDelivery = "real zfb or fixture HTTP response";
browserEnvironment.isolation = "new page per shipping fixture and separate reference origin";
browserEnvironment.requiredMatrixMember = requiredMatrixMember(profile, browserEnvironment);
if (!browserEnvironment.requiredMatrixMember || browserEnvironment.name !== "chromium")
  throw Error(
    `Shipping browser is outside the required checked profile: ${JSON.stringify(browserEnvironment)}`,
  );
const temp = await mkdtemp(join(tmpdir(), "zfb-shipping-"));
const results = new Map();
let sourcePlanRaw;
let referenceScreenshotRaw;
let nodeFreeCssEvidence;
const hash = (value) => createHash("sha256").update(value).digest("hex");
const referenceCompiler = await reference.compile(
  "@theme { --*: initial; --color-brand: #336699; --color-transient: #883344; } @tailwind utilities;",
);
const referenceCss = Buffer.from(referenceCompiler.build(["bg-brand", "text-transient"]));
if (!referenceCss.length) throw Error("SRI-verified Tailwind reference emitted empty CSS");
const referenceRaw = await save("reference", "reference.css", referenceCss);

async function save(id, label, bytes) {
  const name = `${id}-${label}`;
  const path = join(output, name);
  await writeFile(path, bytes);
  return { path: name, sha256: hash(bytes) };
}

async function record(id, generatedCss, servedCss, observed, response, screenshot = null) {
  const requirement = manifest.required.find((row) => row.id === id);
  if (!requirement) throw Error(`Unknown shipping case ${id}`);
  if (
    !servedCss.length ||
    (generatedCss && (!generatedCss.length || !generatedCss.equals(servedCss)))
  )
    throw Error(`Missing or mismatched generated/served CSS: ${id}`);
  if (response.status !== 200 || response.contentType !== "text/css; charset=utf-8")
    throw Error(`CSS HTTP response invalid: ${id}: ${JSON.stringify(response)}`);
  if (digest(observed) !== digest(requirement.expectedObservation))
    throw Error(`Shipping observation failed: ${id}: ${JSON.stringify(observed)}`);
  const raw = {
    generatedCss: generatedCss ? await save(id, "generated.css", generatedCss) : null,
    servedCss: await save(id, "served.css", servedCss),
    observation: null,
    screenshot: screenshot ? await save(id, "composition.png", screenshot) : null,
  };
  raw.observation = await save(
    id,
    "observation.json",
    JSON.stringify(
      {
        caseId: id,
        engine: requirement.engine,
        observed,
        cssResponse: {
          status: response.status,
          contentType: response.contentType,
          sha256: raw.servedCss.sha256,
        },
      },
      null,
      2,
    ) + "\n",
  );
  results.set(id, {
    id,
    mode: requirement.mode,
    engine: requirement.engine,
    configuration: requirement.configuration,
    fixture: requirement.fixture,
    mechanism: requirement.mechanism,
    outcome: "passed",
    browserSettings: screenshot
      ? {
          viewport:
            id === "build-composition"
              ? { width: 1120, height: 800 }
              : { width: 1280, height: 720 },
          colorScheme: "light",
          reducedMotion: "no-preference",
          deviceScaleFactor: 1,
        }
      : null,
    raw,
  });
}

async function serveCss(css, callback) {
  const server = createServer((request, response) => {
    if (request.url === "/style.css") {
      response.writeHead(200, { "Content-Type": "text/css; charset=utf-8" });
      response.end(css);
    } else if (request.url === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(
        '<!doctype html><html><head><link rel="stylesheet" href="/style.css"></head><body><div id="probe" class="bg-brand text-transient">probe</div></body></html>',
      );
    } else {
      response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
      response.end("Not Found");
    }
  });
  await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
  const port = server.address().port;
  try {
    return await callback(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise((ok) => server.close(ok));
  }
}

function runBinary(cwd, commands) {
  const run = spawnSync(productionBuild.binaryPath, commands, {
    cwd,
    encoding: "utf8",
    timeout: 120_000,
  });
  if (run.error || run.status !== 0)
    throw Error(`zfb ${commands.join(" ")} failed: ${run.error ?? run.stderr}`);
  return run.stdout;
}

async function stopped(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise((resolveStop) => child.once("close", resolveStop));
}

async function cssPass(project, id) {
  const outputCss = join(project, "generated.css");
  runBinary(project, [
    "css",
    "--input",
    "entry.css",
    "--output",
    outputCss,
    "--project-root",
    project,
  ]);
  const css = await readFile(outputCss);
  const fresh = join(temp, `clean-${id}`);
  await cp(project, fresh, {
    recursive: true,
    filter: (path) =>
      !["generated.css", "node-free.css", "dist", ".zfb", ".zfb-build"].includes(basename(path)) &&
      !basename(path).startsWith(".zfb-dev-"),
  });
  runBinary(fresh, [
    "css",
    "--input",
    "entry.css",
    "--output",
    "generated.css",
    "--project-root",
    fresh,
  ]);
  if (!css.equals(await readFile(join(fresh, "generated.css"))))
    throw Error(`Warm CLI CSS differs from clean same-mode output: ${id}`);
  const requirement = manifest.required.find((row) => row.id === id);
  const text = css.toString("utf8");
  const observed = {
    present: requirement.expectedObservation.present.filter((needle) => text.includes(needle)),
    absent: requirement.expectedObservation.absent.filter((needle) => !text.includes(needle)),
  };
  await serveCss(css, async (origin) => {
    const page = await browser.newPage({
      viewport: { width: 1280, height: 720 },
      colorScheme: "light",
      reducedMotion: "no-preference",
      deviceScaleFactor: 1,
    });
    try {
      await page.goto(origin, { waitUntil: "load" });
      let screenshot = null;
      if (id === "css-initial") {
        const windColors = await page.locator("#probe").evaluate((node) => ({
          background: getComputedStyle(node).backgroundColor,
          color: getComputedStyle(node).color,
        }));
        await serveCss(referenceCss, async (referenceOrigin) => {
          const referencePage = await browser.newPage({
            viewport: { width: 1280, height: 720 },
            colorScheme: "light",
            reducedMotion: "no-preference",
            deviceScaleFactor: 1,
          });
          try {
            await referencePage.goto(referenceOrigin, { waitUntil: "load" });
            const referenceColors = await referencePage.locator("#probe").evaluate((node) => ({
              background: getComputedStyle(node).backgroundColor,
              color: getComputedStyle(node).color,
            }));
            if (digest(windColors) !== digest(referenceColors))
              throw Error(
                `Independent reference utility styles differ: ${JSON.stringify({ windColors, referenceColors })}`,
              );
            screenshot = await page.locator("#probe").screenshot();
            const referenceScreenshot = await referencePage.locator("#probe").screenshot();
            if (!screenshot.equals(referenceScreenshot))
              throw Error("Isolated utility/control pixels differ from the independent reference");
            referenceScreenshotRaw = await save("reference", "utility.png", referenceScreenshot);
          } finally {
            await referencePage.close();
          }
        });
      }
      const response = await page.request.get(`${origin}/style.css`);
      const served = await response.body();
      await record(
        id,
        css,
        served,
        observed,
        { status: response.status(), contentType: response.headers()["content-type"] },
        screenshot,
      );
    } finally {
      await page.close();
    }
  });
  return css;
}

async function cliSequence() {
  const source = fromRoot("tests/wind-compatibility/shipping/fixtures/cli");
  const project = join(temp, "cli");
  await cp(source, project, { recursive: true });
  const audit = runBinary(project, [
    "wind",
    "audit",
    "--plan",
    "standalone",
    "--project-root",
    project,
  ]);
  if (
    !audit.includes("src") ||
    !audit.includes("@fixture/shipping-ui") ||
    !audit.includes("manifest wind-shipping-fixture")
  )
    throw Error("Shipping source plan omitted configured source or package root");
  sourcePlanRaw = await save("source", "plan.txt", audit);
  const initialCss = await cssPass(project, "css-initial");
  const { NODE_PATH: _nodePath, ...nodeFreeEnvironment } = process.env;
  const nodeFreeArgs = [
    "css",
    "--input",
    "entry.css",
    "--output",
    "node-free.css",
    "--project-root",
    project,
  ];
  const nodeFree = spawnSync(productionBuild.binaryPath, nodeFreeArgs, {
    cwd: project,
    encoding: "utf8",
    timeout: 120_000,
    env: { ...nodeFreeEnvironment, PATH: "" },
  });
  if (nodeFree.error || nodeFree.status !== 0)
    throw Error(`Production zfb css needs a PATH executable: ${nodeFree.error ?? nodeFree.stderr}`);
  const nodeFreeCss = await readFile(join(project, "node-free.css"));
  if (!nodeFreeCss.length || !nodeFreeCss.equals(initialCss))
    throw Error("Node-free zfb css differs from normal current-source CSS");
  nodeFreeCssEvidence = {
    args: nodeFreeArgs.map((arg) => (arg === project ? "<project>" : arg)),
    environment: { PATH: "", NODE_PATH: null },
    exitCode: nodeFree.status,
    rawCss: await save("node-free", "cli.css", nodeFreeCss),
    stdout: await save("node-free", "stdout.txt", nodeFree.stdout),
    stderr: await save("node-free", "stderr.txt", nodeFree.stderr),
  };
  const added = join(project, "src/added.tsx");
  await writeFile(added, 'export const added = "bg-added";\n');
  await cssPass(project, "source-add");
  await writeFile(added, 'export const edited = "bg-edited";\n');
  await cssPass(project, "source-edit");
  await rm(added);
  await cssPass(project, "source-remove");
  await rename(join(project, "src/rename-before.tsx"), join(project, "src/rename-after.tsx"));
  await writeFile(
    join(project, "src/rename-after.tsx"),
    'export const renameClass = "bg-renamed";\n',
  );
  await cssPass(project, "source-rename");
  const configPath = join(project, "zfb.config.json");
  const config = await readJson(configPath);
  config.wind.tokens.colors.brand = "#224466";
  await writeFile(configPath, JSON.stringify(config));
  await cssPass(project, "token-change");
  delete config.wind.tokens.colors.transient;
  await writeFile(join(project, "src/index.tsx"), 'export const shippingClass = "bg-brand";\n');
  await writeFile(configPath, JSON.stringify(config));
  await cssPass(project, "token-removal");
}

async function buildCases() {
  const server = spawn(
    process.execPath,
    [fromRoot("tests/wind-real-build/serve-dist.mjs"), "4332"],
    {
      cwd: fromRoot("."),
      env: {
        ...process.env,
        ZFB_WIND_REAL_BUILD_DIST: resolve(args.dist),
        ZFB_WIND_REAL_BUILD_PROOF: resolve(args["dist-proof"]),
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let logs = "";
  server.on("error", (error) => {
    logs += String(error);
  });
  server.stdout.on("data", (data) => {
    logs += data;
  });
  server.stderr.on("data", (data) => {
    logs += data;
  });
  try {
    const origin = "http://127.0.0.1:4332";
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      if (server.exitCode !== null) throw Error(`Dist server exited: ${logs}`);
      if (!logs.includes("READY http://localhost:4332")) {
        await new Promise((ok) => setTimeout(ok, 100));
        continue;
      }
      try {
        ready = (await fetch(origin)).status === 200;
      } catch {
        /* booting */
      }
      if (ready) break;
      await new Promise((ok) => setTimeout(ok, 100));
    }
    if (!ready) throw Error(`Dist server did not become ready: ${logs}`);
    const page = await browser.newPage({
      viewport: { width: 1120, height: 800 },
      colorScheme: "light",
      reducedMotion: "no-preference",
      deviceScaleFactor: 1,
    });
    try {
      await page.goto(origin, { waitUntil: "networkidle" });
      const href = await page.locator('link[rel="stylesheet"]').first().getAttribute("href");
      if (!/^\/assets\/styles-[^/]+\.css$/.test(href))
        throw Error(`No generated build CSS: ${href}`);
      const generated = await readFile(join(resolve(args.dist), href));
      const response = await page.request.get(origin + href);
      const served = await response.body();
      const card = page.locator("#shipping-card");
      const screenshot = await page.locator("#shipping-layout").screenshot();
      const wide = await page
        .locator("#shipping-layout")
        .evaluate((node) => getComputedStyle(node).gridTemplateColumns.split(" ").length === 2);
      const invalid = await page.locator("#shipping-email").evaluate((node) => {
        node.value = "invalid";
        return !node.checkValidity();
      });
      const valid = await page.locator("#shipping-email").evaluate((node) => {
        node.value = "fixture@example.invalid";
        return node.checkValidity();
      });
      const observed = {
        cardBackground: await card.evaluate((node) => getComputedStyle(node).backgroundColor),
        headingFontSize: await card
          .locator("h2")
          .evaluate((node) => getComputedStyle(node).fontSize),
        darkText: await page
          .locator("#shipping-dark")
          .evaluate((node) => getComputedStyle(node).color),
        navDisplay: await page
          .locator("#shipping-nav")
          .evaluate((node) => getComputedStyle(node).display),
        globalMarker: await page
          .locator(".global-authored-marker")
          .evaluate((node) =>
            getComputedStyle(node).getPropertyValue("--wind-global-marker").trim(),
          ),
        moduleMarker: await page
          .locator("[class*='moduleMarker']")
          .evaluate((node) =>
            getComputedStyle(node).getPropertyValue("--wind-module-marker").trim(),
          ),
        mobileStacked: false,
        wideColumns: wide,
        invalidEmailRejected: invalid,
        validEmailAccepted: valid,
        navFocusOutline: "",
        inputFocusOutline: "",
        buttonHoverBackground: "",
      };
      await page.keyboard.press("Tab");
      observed.navFocusOutline = await page
        .locator('#shipping-nav a[href="#shipping-card"]')
        .evaluate((node) => getComputedStyle(node).outlineWidth);
      await page.locator("#shipping-email").focus();
      observed.inputFocusOutline = await page
        .locator("#shipping-email")
        .evaluate((node) => getComputedStyle(node).outlineWidth);
      await page.locator("#shipping-submit").hover();
      observed.buttonHoverBackground = await page
        .locator("#shipping-submit")
        .evaluate((node) => getComputedStyle(node).backgroundColor);
      await page.setViewportSize({ width: 390, height: 800 });
      const cardBox = await card.boundingBox(),
        formBox = await page.locator("#shipping-form").boundingBox();
      observed.mobileStacked = Boolean(
        cardBox && formBox && formBox.y >= cardBox.y + cardBox.height,
      );
      await record(
        "build-composition",
        generated,
        served,
        observed,
        {
          status: response.status(),
          contentType: response.headers()["content-type"],
        },
        screenshot,
      );
      const missing = await page.request.get(`${origin}/assets/__shipping-missing.css`);
      await record(
        "missing-css",
        generated,
        served,
        {
          missingStatus: missing.status(),
          missingContentType: missing.headers()["content-type"],
        },
        { status: response.status(), contentType: response.headers()["content-type"] },
      );
    } finally {
      await page.close();
    }
  } finally {
    if (server.exitCode === null && server.signalCode === null) server.kill("SIGTERM");
    await stopped(server);
  }
}

async function startDev(project) {
  const child = spawn(productionBuild.binaryPath, ["dev", "--port", "0"], {
    cwd: project,
    env: {
      ...process.env,
      PATH: "",
      NODE_PATH: undefined,
      ZFB_ESBUILD_BIN: proof.execution.buildEnvironment.ZFB_ESBUILD_BIN,
    },
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  let log = "";
  child.on("error", (error) => {
    log += String(error);
  });
  child.stdout.on("data", (data) => {
    log = (log + data).slice(-20000);
  });
  child.stderr.on("data", (data) => {
    log = (log + data).slice(-20000);
  });
  async function stop() {
    if (process.platform === "win32") child.kill("SIGTERM");
    else if (child.pid) {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }
    await stopped(child);
  }
  try {
    let origin;
    const deadline = Date.now() + 120_000;
    while (Date.now() < deadline) {
      if (child.exitCode !== null || child.signalCode !== null)
        throw Error(`zfb dev exited: ${log}`);
      const ready = log.split("\n").filter((line) => /ready|Local:/i.test(line));
      const address = ready.flatMap((line) =>
        [...line.matchAll(/https?:\/\/[^\s]+/g)].map((match) => match[0]),
      )[0];
      if (address) {
        try {
          origin = new URL(address).origin;
          const response = await fetch(`${origin}/wind-raw`, { signal: AbortSignal.timeout(1000) });
          if (response.status === 200 && (await response.text()).includes("WIND_SHIPPING_SSR_OK"))
            break;
        } catch {
          /* server is still booting */
        }
      }
      origin = undefined;
      await new Promise((ok) => setTimeout(ok, 250));
    }
    if (!origin) throw Error(`zfb dev request-time route did not start: ${log}`);
    return { origin, stop };
  } catch (error) {
    await stop();
    throw error;
  }
}

async function devCss(origin, present, absent, label) {
  let last = "no response";
  const deadline = Date.now() + 45_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${origin}/assets/styles.css`, {
        signal: AbortSignal.timeout(5000),
      });
      const css = Buffer.from(await response.arrayBuffer());
      const text = css.toString("utf8");
      if (
        response.status === 200 &&
        present.every((needle) => text.includes(needle)) &&
        absent.every((needle) => !text.includes(needle))
      )
        return {
          css,
          response: { status: response.status, contentType: response.headers.get("content-type") },
        };
      last = `HTTP ${response.status}: ${text.slice(0, 2000)}`;
    } catch (error) {
      last = String(error);
    }
    await new Promise((ok) => setTimeout(ok, 100));
  }
  throw Error(`Dev CSS did not reach ${label}: ${last}`);
}

async function devCases() {
  const project = join(temp, "request-time");
  await cp(fromRoot("crates/zfb/tests/fixtures/embedded-host-request-time"), project, {
    recursive: true,
    filter: (path) =>
      !["dist", ".zfb", ".zfb-build"].includes(basename(path)) &&
      !basename(path).startsWith(".zfb-dev-"),
  });
  const pagePath = join(project, "pages/wind-raw.tsx");
  const original = await readFile(pagePath, "utf8");
  if (!original.includes("WIND_RAW_ROUTE_OK")) throw Error("Request-time fixture changed");
  await writeFile(pagePath, original.replace("WIND_RAW_ROUTE_OK", "WIND_SHIPPING_SSR_OK"));
  const configPath = join(project, "zfb.config.json");
  const config = {
    wind: {
      spec: 1,
      reset: "none",
      tokens: {
        colors: {
          shipping: "#336699",
          added: "#456789",
          edited: "#56789a",
          renamed: "#123456",
          old: "#123455",
          transient: "#883344",
        },
      },
    },
  };
  await writeFile(configPath, JSON.stringify(config));
  await mkdir(join(project, "components"), { recursive: true });
  const oldPath = join(project, "components/shipping.tsx");
  await writeFile(oldPath, 'export const shippingClass = "bg-shipping bg-old text-transient";\n');
  const warm = await startDev(project);
  try {
    const steps = [];
    const stepContract = manifest.required.find((row) => row.id === "dev-stylesheet")
      .expectedObservation.steps;
    async function capture(id, mutationPath, oldPathAbsent = null) {
      const contract = stepContract[steps.length];
      if (contract?.id !== id) throw Error(`Dev step out of order: ${id}`);
      const state = await devCss(warm.origin, contract.present, contract.absent, id);
      let mutationBytes = null;
      try {
        mutationBytes = await readFile(join(project, mutationPath));
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      const mutationSha256 = mutationBytes ? sha256(mutationBytes) : null;
      if (oldPathAbsent) {
        try {
          await readFile(oldPath);
          throw Error("Renamed source old path still exists");
        } catch (error) {
          if (error.code !== "ENOENT") throw error;
        }
      }
      const inputDigest = digest({
        config: sha256(await readFile(configPath)),
        components: await treeDigest(join(project, "components")),
        pages: await treeDigest(join(project, "pages")),
      });
      steps.push({
        id,
        present: contract.present,
        absent: contract.absent,
        mutation: { path: mutationPath, sha256: mutationSha256, oldPathAbsent },
        rawInput: mutationBytes ? await save(`dev-${id}`, "input", mutationBytes) : null,
        inputDigest,
        cssResponse: { ...state.response, sha256: sha256(state.css) },
        rawCss: await save(`dev-${id}`, "served.css", state.css),
      });
      return state;
    }
    await capture("initial", "components/shipping.tsx");
    const added = join(project, "components/added.tsx");
    await writeFile(added, 'export const added = "bg-added";\n');
    await capture("source-add", "components/added.tsx");
    await writeFile(added, 'export const edited = "bg-edited";\n');
    await capture("source-edit", "components/added.tsx");
    await rm(added);
    await capture("source-remove", "components/added.tsx");
    const newPath = join(project, "components/shipping-renamed.tsx");
    await rename(oldPath, newPath);
    await writeFile(
      newPath,
      'export const shippingClass = "bg-shipping bg-renamed text-transient";\n',
    );
    await capture("source-rename", "components/shipping-renamed.tsx", true);
    config.wind.tokens.colors.shipping = "#224466";
    await writeFile(configPath, JSON.stringify(config));
    await capture("token-change", "zfb.config.json");
    delete config.wind.tokens.colors.transient;
    await writeFile(newPath, 'export const shippingClass = "bg-shipping bg-renamed";\n');
    await writeFile(configPath, JSON.stringify(config));
    const final = await capture("token-removal", "zfb.config.json");
    const cleanProject = join(temp, "request-time-clean");
    await cp(project, cleanProject, {
      recursive: true,
      filter: (path) =>
        !["dist", ".zfb", ".zfb-build"].includes(basename(path)) &&
        !basename(path).startsWith(".zfb-dev-"),
    });
    const clean = await startDev(cleanProject);
    let cleanFinalCss;
    try {
      const cleanFinal = await devCss(
        clean.origin,
        [".bg-shipping", ".bg-renamed"],
        [".text-transient"],
        "clean final",
      );
      if (!final.css.equals(cleanFinal.css))
        throw Error("Warm dev stylesheet differs from clean dev stylesheet");
      cleanFinalCss = await save("dev-clean-final", "served.css", cleanFinal.css);
    } finally {
      await clean.stop();
    }
    const page = await browser.newPage();
    try {
      const routeResponse = await page.goto(`${warm.origin}/wind-raw`, { waitUntil: "load" });
      if (routeResponse?.status() !== 200) throw Error("Request-time route returned non-200");
      const style = await page.locator("head > style").textContent();
      await record(
        "dev-stylesheet",
        null,
        final.css,
        {
          steps: steps.map(({ id, present, absent }) => ({ id, present, absent })),
          cleanFinalMatches: true,
        },
        final.response,
      );
      results.get("dev-stylesheet").steps = steps;
      results.get("dev-stylesheet").cleanFinalCss = cleanFinalCss;
      await record(
        "request-time-ssr",
        null,
        final.css,
        {
          routeMarker:
            (await page.locator("#wind-raw-route").textContent()) === "WIND_SHIPPING_SSR_OK",
          quotedCss:
            style ===
            'body[data-zfb-wind="raw"] { font-family: "Wind & Raw"; --quoted-css: "<raw & trusted>"; }',
        },
        final.response,
      );
    } finally {
      await page.close();
    }
  } finally {
    await warm.stop();
  }
}

try {
  await cliSequence();
  await buildCases();
  await devCases();
  const fixtureTreeDigest = await treeDigest(fromRoot(manifest.fixtureRoot));
  const configDigest = digest(
    await Promise.all(
      manifest.configFiles.map(async (path) => [path, sha256(await readFile(fromRoot(path)))]),
    ),
  );
  const report = {
    schemaVersion: 1,
    kind: "wind-shipping-consumer-evidence",
    manifestDigest: sha256(await readFile(fromRoot(shippingManifestPath))),
    testedSourceSha: comparison.testedSourceSha,
    testedInputsDigest: inputs.digest,
    windBuild: comparison.candidate.windBuild,
    productionBuild,
    distProof: proof,
    toolchain: plan.toolchain,
    profileDigest: comparison.profile.digest,
    fixtureTreeDigest,
    configDigest,
    reference: reference.identity,
    referenceCache: resolve(args["reference-cache"]),
    referenceCss: referenceRaw,
    referenceScreenshot: referenceScreenshotRaw,
    sourcePlan: sourcePlanRaw,
    browserEnvironment,
    nodeFreeCss: nodeFreeCssEvidence,
    nodeFreeDevEnvironment: {
      PATH: "",
      NODE_PATH: null,
      ZFB_ESBUILD_BIN: proof.execution.buildEnvironment.ZFB_ESBUILD_BIN,
    },
    results: manifest.required.map((row) => {
      const result = results.get(row.id);
      if (!result) throw Error(`Mandatory shipping case missing: ${row.id}`);
      return result;
    }),
  };
  report.reportId = `sha256:${digest(report)}`;
  await validateShippingEvidence(report, comparison, output, plan.toolchain);
  await writeFile(join(output, "report.json"), JSON.stringify(report, null, 2) + "\n");
  process.stdout.write(`${join(output, "report.json")}\n`);
} finally {
  await browser.close();
  await rm(temp, { recursive: true, force: true });
}
