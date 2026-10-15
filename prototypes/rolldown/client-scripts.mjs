// Real CLI and Chromium proof for standalone client scripts. Run from the repository root.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, extname, isAbsolute, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const repo = resolve(import.meta.dirname, "../..");
const profile = process.env.ZFB_PROTOTYPE_PROFILE ?? "debug";
assert.ok(["debug", "release"].includes(profile));
const binary = resolve(repo, `target/${profile}/zfb`);
assert.ok(existsSync(binary), `prepared CLI missing: ${binary}`);
assert.ok(
  isAbsolute(process.env.ZFB_ESBUILD_BIN ?? "") && existsSync(process.env.ZFB_ESBUILD_BIN),
  "ZFB_ESBUILD_BIN must name an absolute installed esbuild binary",
);
const gate = spawnSync(process.execPath, [join(import.meta.dirname, "gate.mjs")], {
  cwd: repo,
  env: process.env,
  encoding: "utf8",
  timeout: 30_000,
});
assert.equal(gate.status, 0, `prepared Rolldown gate failed: ${gate.stderr}`);
const tools = resolve(process.env.ZFB_PROTOTYPE_TOOLS ?? join(repo, "target/rolldown-tools"));
const { chromium } = await import(pathToFileURL(join(tools, "node_modules/playwright/index.mjs")));
const fixture = join(import.meta.dirname, "client-fixture");
const output = join(repo, "target/rolldown-client-scripts");
const deadlineMs = 30_000;

function prepare(root) {
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  cpSync(fixture, root, { recursive: true });
  for (const dir of ["content", "layouts", "node_modules/@takazudo"])
    mkdirSync(join(root, dir), { recursive: true });
  for (const name of ["zfb", "zfb-runtime", "zfb-slugify"]) {
    const source = join(repo, "packages", name);
    const destination = join(root, "node_modules/@takazudo", name);
    cpSync(source, destination, { recursive: true });
  }
  cpSync(join(tools, "node_modules/hono"), join(root, "node_modules/hono"), {
    recursive: true,
  });
}

function environment(backend) {
  const env = { ...process.env, ZFB_DEV_TIMING: "1" };
  delete env.ZFB_ROLLDOWN_PROTOTYPE;
  delete env.ZFB_DEV_EAGER;
  delete env.ZFB_LAZY_DEV_RENDER;
  delete env.ZFB_DEV_DEFER_BUNDLE;
  if (backend === "rolldown") env.ZFB_ROLLDOWN_PROTOTYPE = "1";
  return env;
}

function cli(root, env, shouldPass = true) {
  const result = spawnSync(binary, ["build"], {
    cwd: root,
    env,
    encoding: "utf8",
    timeout: 120_000,
  });
  assert.equal(result.error, undefined, String(result.error));
  assert.equal(
    result.status === 0,
    shouldPass,
    `zfb build status=${result.status}\n${result.stdout}\n${result.stderr}`,
  );
  return result;
}

function attribution(stderr, backend, phase, workerStems) {
  const lines = stderr
    .split(/\r?\n/)
    .filter((line) => line.includes("rolldown-prototype client-script job="));
  const jobs = lines.map((line) => {
    const match = line.match(
      /\[zfb-timing\] rolldown-prototype client-script job=([^\s]+) kind=(entry|worker)\b/,
    );
    assert.ok(match, `${phase}: malformed attribution: ${line}`);
    return { stem: match[1], kind: match[2] };
  });
  if (backend === "esbuild") assert.deepEqual(jobs, [], `${phase}: native dispatch on esbuild`);
  else {
    assert.deepEqual(
      jobs.map((job) => job.stem).sort(),
      ["probe", ...workerStems].sort(),
      `${phase}: native jobs`,
    );
    assert.deepEqual(
      jobs.map((job) => `${job.stem}:${job.kind}`).sort(),
      ["probe:entry", ...workerStems.map((stem) => `${stem}:worker`)].sort(),
      `${phase}: native job kinds`,
    );
  }
  return { count: jobs.length, jobs };
}

function entryFromHtml(root) {
  const html = readFileSync(join(root, "dist/index.html"), "utf8");
  const urls = [...html.matchAll(/\/assets\/client\/probe-[0-9a-f]+\.js/g)].map(
    (match) => match[0],
  );
  assert.equal(urls.length, 1, `production HTML must name one hashed entry: ${html}`);
  return urls[0];
}

function productionAssets(root, entryUrl) {
  const dir = join(root, "dist/assets/client");
  const names = readdirSync(dir).sort();
  const entryName = entryUrl.split("/").at(-1);
  assert.equal(names.length, 3, `client asset namespace: ${names}`);
  assert.ok(names.includes(entryName));
  const workers = names.filter((name) => name !== entryName);
  assert.ok(
    workers.every((name) => /^worker-[^/]+\.js$/.test(name)),
    `worker companions: ${workers}`,
  );
  const entry = readFileSync(join(dir, entryName), "utf8");
  assert.ok(entry.includes("LAZY_INLINED_VALUE"), "lazy module must be inlined into entry");
  for (const name of workers)
    assert.ok(
      !readFileSync(join(dir, name), "utf8").includes("LAZY_INLINED_VALUE"),
      `lazy value leaked to ${name}`,
    );
  return { entryUrl, names, workerStems: workers.map((name) => name.slice(0, -3)) };
}

async function staticServer(dist) {
  const server = createServer((request, response) => {
    try {
      const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
      const file = resolve(dist, `.${pathname === "/" ? "/index.html" : pathname}`);
      assert.ok(file.startsWith(dist + sep), "static path outside dist");
      response.setHeader(
        "Content-Type",
        {
          ".html": "text/html",
          ".js": "text/javascript",
        }[extname(file)] ?? "application/octet-stream",
      );
      response.end(readFileSync(file));
    } catch {
      response.statusCode = 404;
      response.end("not found");
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    async close() {
      server.closeAllConnections();
      await new Promise((done) => server.close(done));
    },
  };
}

async function inspectPage(browser, origin, workerValue) {
  const page = await browser.newPage();
  const errors = [];
  const assetResponses = [];
  page.on("pageerror", (error) => errors.push(`page: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  page.on("requestfailed", (request) =>
    errors.push(`request: ${request.url()} ${request.failure()?.errorText}`),
  );
  page.on("response", (response) => {
    if (new URL(response.url()).pathname.startsWith("/assets/client/"))
      assetResponses.push({ url: response.url(), status: response.status() });
  });
  try {
    const response = await page.goto(origin, { waitUntil: "load", timeout: deadlineMs });
    assert.equal(response.status(), 200);
    await page.waitForFunction(
      ({ workerValue }) =>
        document.querySelector("#entry")?.textContent === "ENTRY_READY" &&
        document.querySelector("#lazy")?.textContent === "LAZY_INLINED_VALUE" &&
        document.querySelector("#worker")?.textContent === workerValue &&
        document.querySelector("#nested")?.textContent === "NESTED_WORKER_READY",
      { workerValue },
      { timeout: deadlineMs },
    );
    assert.equal(await page.locator("#worker-error").textContent(), "");
    assert.ok(
      assetResponses.length >= 3,
      `expected entry and two worker responses: ${JSON.stringify(assetResponses)}`,
    );
    assert.ok(
      assetResponses.every(({ status }) => status >= 200 && status < 300),
      JSON.stringify(assetResponses),
    );
    assert.deepEqual(errors, []);
    return { assetResponses, browserErrors: errors.length };
  } finally {
    await page.close();
  }
}

function waitForCondition(check, label, child) {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const poll = async () => {
      try {
        if (child?.exitCode !== null)
          throw new Error(`dev exited before ${label}: ${child.exitCode}`);
        const value = await check();
        if (value) return resolve(value);
        if (Date.now() - started >= deadlineMs) throw new Error(`timeout waiting for ${label}`);
        setTimeout(poll, 50);
      } catch (error) {
        reject(error);
      }
    };
    poll();
  });
}

async function subscribeSse(origin) {
  const controller = new AbortController();
  const openDeadline = setTimeout(() => controller.abort(), deadlineMs);
  let response;
  try {
    // The ready banner precedes the first route publication. During that
    // short interval the dev server answers 404 for its reload endpoint.
    do {
      response = await fetch(new URL("__zfb/reload", origin), { signal: controller.signal });
      if (response.status === 200) break;
      assert.equal(response.status, 404, "SSE startup status");
      await response.body?.cancel();
      await new Promise((done) => setTimeout(done, 50));
    } while (!controller.signal.aborted);
  } finally {
    clearTimeout(openDeadline);
  }
  assert.equal(response.status, 200, "SSE status");
  assert.match(response.headers.get("content-type") ?? "", /text\/event-stream/);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const events = [];
  let failure;
  const task = (async () => {
    let buffer = "";
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) throw new Error("SSE closed");
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
        let end;
        while ((end = buffer.indexOf("\n\n")) !== -1) {
          const frame = buffer.slice(0, end);
          buffer = buffer.slice(end + 2);
          const name = frame.match(/^event: (.+)$/m)?.[1];
          if (name) events.push(name);
        }
      }
    } catch (error) {
      if (!controller.signal.aborted) failure = error;
    }
  })();
  return {
    events,
    get failure() {
      return failure;
    },
    async close() {
      controller.abort();
      await task;
    },
  };
}

function strayTemps(root) {
  const found = [];
  function visit(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (
        /^\.zfb-(?:client-script|worker|esbuild|rolldown|stage-audit|islands-tsconfig|virtual)/.test(
          entry.name,
        )
      )
        found.push(join(dir, entry.name));
      if (entry.isDirectory() && !["node_modules", "dist"].includes(entry.name))
        visit(join(dir, entry.name));
    }
  }
  visit(root);
  return found;
}

async function development(browser, root, env, backend, workerStems) {
  const child = spawn(binary, ["dev", "--port", "0"], {
    cwd: root,
    env,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr += chunk;
  });
  let sse;
  try {
    const port = await waitForCondition(
      () => {
        const match = stdout.match(/ready on http:\/\/[^:\s]+:(\d+)/);
        return match && Number(match[1]);
      },
      "dev ready banner",
      child,
    );
    const origin = `http://127.0.0.1:${port}/`;
    sse = await subscribeSse(origin);
    const initial = await inspectPage(browser, origin, "WORKER_VALUE_V1");
    console.log(`${backend}: dev initial Chromium execution PASS`);
    const stderrOffset = stderr.length;
    const eventOffset = sse.events.length;
    writeFileSync(
      join(root, "components/worker-dep.ts"),
      'export const workerValue = "WORKER_VALUE_V2";\n',
    );
    await waitForCondition(
      () => {
        if (sse.failure) throw sse.failure;
        return sse.events.slice(eventOffset).some((name) => ["page", "islands"].includes(name));
      },
      "worker edit SSE reload event",
      child,
    );
    const workerName = await waitForCondition(
      () => {
        const dir = join(root, ".zfb-build/dev-assets/assets/client");
        try {
          return readdirSync(dir).find(
            (name) =>
              /^worker-.*\.js$/.test(name) &&
              readFileSync(join(dir, name), "utf8").includes("WORKER_VALUE_V2"),
          );
        } catch (error) {
          if (error.code === "ENOENT") return false;
          throw error;
        }
      },
      "published worker file containing edited value",
      child,
    );
    await waitForCondition(
      async () => {
        const response = await fetch(`${origin}assets/client/${workerName}`);
        return response.status === 200 && (await response.text()).includes("WORKER_VALUE_V2");
      },
      "published generation's worker asset",
      child,
    );
    const updated = await inspectPage(browser, origin, "WORKER_VALUE_V2");
    const editJobs = attribution(stderr.slice(stderrOffset), backend, "dev edit", workerStems);
    console.log(
      `${backend}: one worker dependency edit, SSE, refreshed assets, Chromium, native attribution PASS`,
    );
    return { initial, updated, reloadEvents: sse.events.slice(eventOffset), attribution: editJobs };
  } finally {
    if (sse) await sse.close();
    if (child.exitCode === null) {
      const exited = new Promise((done) => child.once("exit", done));
      process.kill(-child.pid, "SIGTERM");
      await exited;
    }
    assert.deepEqual(strayTemps(root), [], "stranded temporary client stages");
  }
}

const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.ZFB_CHROMIUM_EXECUTABLE,
});
console.log(`Chromium ${browser.version()}`);
try {
  for (const backend of ["esbuild", "rolldown"]) {
    const root = join(output, backend, "project");
    const evidence = join(output, backend, "evidence");
    prepare(root);
    rmSync(evidence, { recursive: true, force: true });
    mkdirSync(evidence, { recursive: true });
    const env = environment(backend);
    const build = cli(root, env);
    const assets = productionAssets(root, entryFromHtml(root));
    const buildJobs = attribution(build.stderr, backend, "production", assets.workerStems);
    const server = await staticServer(join(root, "dist"));
    let production;
    try {
      production = await inspectPage(browser, server.origin, "WORKER_VALUE_V1");
    } finally {
      await server.close();
    }
    console.log(`${backend}: production CLI, assets, Chromium, native attribution PASS`);
    const dev = await development(browser, root, env, backend, assets.workerStems);
    const missingRoot = join(output, backend, "missing-import");
    prepare(missingRoot);
    writeFileSync(
      join(missingRoot, "components/probe.client.ts"),
      'import "./missing-client-import.ts";\n' +
        readFileSync(join(missingRoot, "components/probe.client.ts"), "utf8"),
    );
    const missing = cli(missingRoot, env, false);
    assert.match(missing.stderr, /missing-client-import\.ts/);
    const escapeRoot = join(output, backend, "worker-escape");
    prepare(escapeRoot);
    // A claimed workspace widens the first-party boundary to the parent.
    // The worker dependency below is inside that boundary but outside the
    // staged project, so the CLI arms and exercises the per-worker audit.
    writeFileSync(
      join(dirname(escapeRoot), "pnpm-workspace.yaml"),
      "packages:\n  - 'worker-escape'\n",
    );
    const outside = join(dirname(escapeRoot), "outside-worker-dep.ts");
    writeFileSync(outside, 'export const workerValue = "OUTSIDE";\n');
    writeFileSync(
      join(escapeRoot, "components/worker.ts"),
      readFileSync(join(escapeRoot, "components/worker.ts"), "utf8").replace(
        '"./worker-dep.ts"',
        JSON.stringify(outside),
      ),
    );
    const escape = cli(escapeRoot, env, false);
    assert.match(escape.stderr, /stage-escape audit failed for client-script module worker/);
    assert.match(escape.stderr, /worker\.ts/);
    assert.match(escape.stderr, /outside-worker-dep\.ts/);
    console.log(`${backend}: missing import and worker stage escape CLI negatives PASS`);
    writeFileSync(
      join(evidence, "report.json"),
      JSON.stringify(
        {
          backend,
          profile,
          platform: process.platform,
          arch: process.arch,
          cli: binary,
          production: { assets, ...production, attribution: buildJobs },
          development: dev,
          negatives: { missingImport: missing.status, workerStageEscape: escape.status },
        },
        null,
        2,
      ) + "\n",
    );
  }
} finally {
  await browser.close();
}
