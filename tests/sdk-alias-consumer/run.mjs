#!/usr/bin/env node
// Confirmation-only fresh-consumer proof for #3937. The #3939 health workflow
// runs it after the Rust and scanner-browser checks, outside the fast workspace
// test command. It needs `ZFB_BINARY` and `ZFB_BINARY_SOURCE_SHA` for a workspace
// binary built from that exact checkout.

import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import {
  chmod,
  copyFile,
  mkdtemp,
  readFile,
  readdir,
  rm,
  mkdir,
  writeFile,
} from "node:fs/promises";
import net from "node:net";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "../..");
const DOCS_RECIPE = join(REPO_ROOT, "docs/src/content/docs/concepts/mdx-components.mdx");
const SHIM_SOURCE = join(REPO_ROOT, "crates/zfb/templates/basic-blog/components/zfb-shim.d.ts");
const TYPESCRIPT_VERSION = "7.0.2";
const DEV_TIMEOUT_MS = 90_000;

function currentSourceSha() {
  return execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
  }).trim();
}

async function copyCurrentBinary(destination) {
  const requestedBinary = process.env.ZFB_BINARY;
  const binarySourceSha = process.env.ZFB_BINARY_SOURCE_SHA;
  assert.ok(requestedBinary, "set ZFB_BINARY to a binary built from this checkout");
  assert.equal(
    binarySourceSha,
    currentSourceSha(),
    "ZFB_BINARY_SOURCE_SHA must match the current checkout SHA",
  );
  const source = isAbsolute(requestedBinary)
    ? requestedBinary
    : resolve(REPO_ROOT, requestedBinary);
  await copyFile(source, destination);
  await chmod(destination, 0o755);
  process.stdout.write(`Using current-source zfb binary: ${source}\n`);
  return destination;
}

function parseFencedBlocks(markdown) {
  const blocks = [];
  const fence = /^```([^\r\n]*)\r?\n([\s\S]*?)^```[ \t]*$/gm;
  for (const match of markdown.matchAll(fence)) {
    blocks.push({ language: match[1].trim(), source: match[2].replace(/\r?\n$/, "") });
  }
  return blocks;
}

function recipeBlock(blocks, fileMarker, requiredSource) {
  const matches = blocks.filter(({ language, source }) => {
    return (
      language === "tsx" &&
      source.trimStart().startsWith(`// ${fileMarker}`) &&
      (!requiredSource || source.includes(requiredSource))
    );
  });
  assert.equal(
    matches.length,
    1,
    `expected one fenced tsx recipe for ${fileMarker} in ${DOCS_RECIPE}; found ${matches.length}`,
  );
  return matches[0].source;
}

function assertBareSdkImports(source, fileMarker) {
  assert.ok(source.includes('from "zfb"') || source.includes('from "zfb/'), fileMarker);
  assert.doesNotMatch(
    source,
    /@takazudo\/zfb/u,
    `${fileMarker} must use the documented bare alias`,
  );
}

function extractPageImportsAndContent(source) {
  const imports = source
    .split("\n")
    .filter((line) => /^import\s/u.test(line))
    .join("\n");
  const contentCall = source.match(/<post\.Content[\s\S]*?\/>/u)?.[0];
  assert.ok(imports && contentCall, "the page recipe needs its imports and post.Content call");
  return { imports, contentCall };
}

async function run(command, args, { cwd, label }) {
  process.stdout.write(`\n==> ${label}\n`);
  await new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { cwd, stdio: "inherit" });
    child.once("error", rejectRun);
    child.once("exit", (code, signal) => {
      if (code === 0) resolveRun();
      else rejectRun(new Error(`${label} failed (${signal ?? `exit ${code}`})`));
    });
  });
}

async function packPackage(packageDir, tarballDir, label) {
  await run("pnpm", ["run", "build"], { cwd: packageDir, label: `build ${label}` });
  await run("pnpm", ["pack", "--pack-destination", tarballDir], {
    cwd: packageDir,
    label: `pack ${label}`,
  });
  const expectedPrefix = label.replace(/^@takazudo\//u, "takazudo-").replaceAll("/", "-");
  const archives = (await readdir(tarballDir)).filter(
    (name) => name.startsWith(`${expectedPrefix}-`) && name.endsWith(".tgz"),
  );
  assert.equal(archives.length, 1, `expected one packed archive for ${label}`);
  return join(tarballDir, archives[0]);
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address === "object");
  await new Promise((resolveClose, rejectClose) => {
    server.close((error) => (error ? rejectClose(error) : resolveClose()));
  });
  return address.port;
}

function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

async function poll(description, probe, timeoutMs = DEV_TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const result = await probe();
      if (result) return result;
    } catch (error) {
      if (error.fatal) throw error;
      lastError = error;
    }
    await sleep(300);
  }
  throw new Error(
    `timed out waiting for ${description}${lastError ? `: ${lastError}${lastError.cause ? ` (cause: ${lastError.cause})` : ""}` : ""}`,
    { cause: lastError },
  );
}

async function fetchText(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(5000) });
  return { response, text: await response.text() };
}

function signalProcessGroup(child, signal) {
  if (process.platform === "win32") {
    child.kill(signal);
    return;
  }
  if (!child.pid) return;
  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function startDev(consumer, zfbBinary, port) {
  const child = spawn(zfbBinary, ["dev", "--host", "127.0.0.1", "--port", String(port)], {
    cwd: consumer,
    env: { ...process.env, ZFB_DEV_DEFER_BUNDLE: "0" },
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  let logs = "";
  const capture = (chunk) => {
    const text = chunk.toString();
    logs = (logs + text).slice(-60_000);
    process.stdout.write(text);
  };
  child.stdout.on("data", capture);
  child.stderr.on("data", capture);
  child.once("exit", (code, signal) => {
    capture(`\nzfb dev exited: code=${code} signal=${signal}\n`);
  });
  let spawnError;
  child.on("error", (error) => {
    spawnError = error;
    logs = (logs + `\n${error}\n`).slice(-60_000);
  });
  return {
    child,
    logs: () => logs,
    spawnError: () => spawnError,
    async stop() {
      if (child.exitCode !== null || child.signalCode !== null) return;
      signalProcessGroup(child, "SIGTERM");
      await Promise.race([once(child, "exit"), sleep(10_000)]);
      if (child.exitCode === null && child.signalCode === null) {
        signalProcessGroup(child, "SIGKILL");
      }
    },
  };
}

async function writeConsumer(consumer, docsBlocks) {
  const counter = recipeBlock(docsBlocks, "components/counter.tsx");
  const rootComponents = recipeBlock(
    docsBlocks,
    "mdx-components.tsx (project root)",
    'import { Island } from "zfb";',
  );
  const pageComponents = recipeBlock(docsBlocks, "pages/_mdx-components.tsx");
  const page = recipeBlock(docsBlocks, "pages/blog/[slug].tsx", "pageComponents");
  assertBareSdkImports(counter, "components/counter.tsx");
  assertBareSdkImports(rootComponents, "mdx-components.tsx");
  assertBareSdkImports(pageComponents, "pages/_mdx-components.tsx");

  const { imports, contentCall } = extractPageImportsAndContent(page);
  // The docs fence shows the dynamic blog route's relative imports. This
  // disposable consumer uses the root route, so adapt only those local paths.
  const rootPageImports = imports
    .replace("../../mdx-components", "../mdx-components")
    .replace("../_mdx-components", "./_mdx-components");
  const pageSource = `import { getCollection } from "zfb/content";\n${rootPageImports}\n\nexport default function HomePage() {
  const post = getCollection("blog").find((entry) => entry.slug === "alias-recipe");
  if (!post) return <html><body><p>Missing recipe entry</p></body></html>;
  return (
    <html lang="en">
      <head><title>SDK alias recipe</title></head>
      <body><main>${contentCall}</main></body>
    </html>
  );
}
`;

  await mkdir(join(consumer, "components"), { recursive: true });
  await mkdir(join(consumer, "pages"), { recursive: true });
  await mkdir(join(consumer, "content/blog"), { recursive: true });
  await writeFile(join(consumer, "components/counter.tsx"), `${counter}\n`);
  await writeFile(join(consumer, "mdx-components.tsx"), `${rootComponents}\n`);
  await writeFile(join(consumer, "pages/_mdx-components.tsx"), `${pageComponents}\n`);
  await writeFile(join(consumer, "pages/index.tsx"), pageSource);
  await writeFile(
    join(consumer, "content/blog/alias-recipe.mdx"),
    `---\ntitle: SDK alias recipe\n---\n\n<Counter />\n\n<NamedCounter />\n`,
  );

  await writeFile(join(consumer, "components/zfb-shim.d.ts"), await readFile(SHIM_SOURCE, "utf8"));
  await writeFile(
    join(consumer, "tsconfig.json"),
    `${JSON.stringify(
      {
        compilerOptions: {
          target: "ES2022",
          module: "ESNext",
          moduleResolution: "Bundler",
          lib: ["ES2022", "DOM", "DOM.Iterable"],
          jsx: "react-jsx",
          jsxImportSource: "zfb/zudo-react",
          strict: true,
          noEmit: true,
          skipLibCheck: true,
        },
        include: ["**/*.ts", "**/*.tsx", "components/zfb-shim.d.ts"],
      },
      null,
      2,
    )}\n`,
  );
  await writeFile(
    join(consumer, "zfb.config.json"),
    `${JSON.stringify(
      { wind: false, collections: [{ name: "blog", path: "content/blog" }] },
      null,
      2,
    )}\n`,
  );
}

async function runFreshConsumer(consumer, zfbBinary, origin) {
  const compiler = join(consumer, "node_modules/typescript/bin/tsc");
  await run(process.execPath, [compiler, "--noEmit"], { cwd: consumer, label: "consumer tsc" });
  await run(zfbBinary, ["check"], { cwd: consumer, label: "consumer zfb check" });
  await run(zfbBinary, ["build"], { cwd: consumer, label: "consumer zfb build" });

  const port = Number(new URL(origin).port);
  const server = startDev(consumer, zfbBinary, port);
  try {
    const pageUrl = `${origin}/`;
    await poll("zfb dev page readiness", async () => {
      if (server.spawnError()) throw server.spawnError();
      if (server.child.exitCode !== null || server.child.signalCode !== null) {
        const error = new Error(`zfb dev exited early\n${server.logs()}`);
        error.fatal = true;
        throw error;
      }
      const { response, text } = await fetchText(pageUrl);
      if (!response.ok)
        throw new Error(`dev page returned ${response.status}: ${text.slice(0, 300)}`);
      return text.includes("Count: 0") && text.includes("Named counter: 0");
    }).catch((error) => {
      throw new Error(
        `${error.message}\nDev status: code=${server.child.exitCode} signal=${server.child.signalCode}\nServer log:\n${server.logs()}`,
        { cause: error },
      );
    });

    const assetUrl = `${origin}/assets/islands.js`;
    const initialAsset = await poll("initial dev islands bundle", async () => {
      const { response, text } = await fetchText(assetUrl);
      return response.ok && text.includes("Named counter") ? text : null;
    });
    const componentPath = join(consumer, "components/counter.tsx");
    const originalComponent = await readFile(componentPath, "utf8");
    assert.ok(
      originalComponent.includes("Count: {count}"),
      "recipe component label must be present",
    );
    const editedComponent = originalComponent.replace("Count: {count}", "Edited counter: {count}");
    assert.notEqual(
      editedComponent,
      originalComponent,
      "the single client edit must change source",
    );
    await writeFile(componentPath, editedComponent);

    await poll("client island rebundle and dev tick", async () => {
      const [
        { response: pageResponse, text: pageText },
        { response: assetResponse, text: assetText },
      ] = await Promise.all([fetchText(pageUrl), fetchText(assetUrl)]);
      return (
        pageResponse.ok &&
        assetResponse.ok &&
        pageText.includes("Edited counter: 0") &&
        assetText.includes("Edited counter") &&
        assetText !== initialAsset
      );
    });

    const browser = await chromium.launch({ headless: true });
    try {
      const page = await browser.newPage();
      const identityErrors = [];
      page.on("console", (message) => {
        if (message.type() === "error" && /ZR_ISLAND_IDENTITY|ZR_IDENTITY/u.test(message.text())) {
          identityErrors.push(message.text());
        }
      });
      const response = await page.goto(pageUrl, { waitUntil: "domcontentloaded" });
      assert.equal(response?.status(), 200, `unexpected response from ${pageUrl}`);
      await page.waitForFunction(() => {
        const islands = [...document.querySelectorAll("[data-zfb-island]")];
        return (
          islands.length === 2 &&
          islands.every((island) => island.hasAttribute("data-zfb-island-mounted"))
        );
      });
      const markers = await page
        .locator("[data-zfb-island]")
        .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-zfb-island")).sort());
      assert.deepEqual(markers, ["Counter", "NamedCounter"]);

      const counter = page.locator("button").nth(0);
      const namedCounter = page.locator("button").nth(1);
      assert.equal((await counter.textContent())?.trim(), "Edited counter: 0");
      assert.equal((await namedCounter.textContent())?.trim(), "Named counter: 0");
      await counter.click();
      await namedCounter.click();
      await page.waitForFunction(() => {
        const buttons = [...document.querySelectorAll("button")];
        return (
          buttons[0]?.textContent?.trim() === "Edited counter: 1" &&
          buttons[1]?.textContent?.trim() === "Named counter: 1"
        );
      });
      assert.deepEqual(identityErrors, [], "hydration logged an SDK identity error");
      process.stdout.write("\nFresh packed consumer passed hydration and both island clicks.\n");
    } finally {
      await browser.close();
    }
  } finally {
    await server.stop();
  }
}

async function main() {
  const docsBlocks = parseFencedBlocks(await readFile(DOCS_RECIPE, "utf8"));
  const tempRoot = await mkdtemp(join(tmpdir(), "zfb-sdk-alias-consumer-"));
  const tarballDir = join(tempRoot, "tarballs");
  const consumer = join(tempRoot, "consumer");

  try {
    assert.ok(
      !resolve(tempRoot).startsWith(`${REPO_ROOT}${sep}`),
      `temporary consumer directory must be outside the repository: ${tempRoot}`,
    );
    await mkdir(tarballDir, { recursive: true });
    await mkdir(consumer, { recursive: true });
    const zfbBinary = await copyCurrentBinary(join(tempRoot, "zfb-bin"));
    const zfbTarball = await packPackage(
      join(REPO_ROOT, "packages/zfb"),
      tarballDir,
      "@takazudo/zfb",
    );
    const runtimeTarball = await packPackage(
      join(REPO_ROOT, "packages/zfb-runtime"),
      tarballDir,
      "@takazudo/zfb-runtime",
    );
    await writeConsumer(consumer, docsBlocks);
    await writeFile(
      join(consumer, "package.json"),
      `${JSON.stringify(
        {
          name: "zfb-sdk-alias-fresh-consumer",
          private: true,
          type: "module",
          dependencies: {
            "@takazudo/zfb": `file:${zfbTarball}`,
            "@takazudo/zfb-runtime": `file:${runtimeTarball}`,
          },
          devDependencies: { typescript: TYPESCRIPT_VERSION },
        },
        null,
        2,
      )}\n`,
    );

    await run("npm", ["install", "--no-audit", "--no-fund"], {
      cwd: consumer,
      label: "install packed consumer dependencies",
    });
    await runFreshConsumer(consumer, zfbBinary, `http://127.0.0.1:${await freePort()}`);
  } finally {
    await rm(tempRoot, { recursive: true, force: true });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error}\n`);
  process.exitCode = 1;
});
