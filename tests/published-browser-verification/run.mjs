/** @verification: installed registry evidence for #3879; manager-owned browser/build lane. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { mkdir, readFile, writeFile, copyFile, readdir, realpath } from "node:fs/promises";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { platform, release, arch } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const output = resolve(process.argv[2] ?? "");
assert.ok(process.argv[2], "usage: node run.mjs /absolute/output-directory");
assert.equal(platform(), "linux");
assert.equal(arch(), "x64", "frozen native carrier is Linux x64 glibc");
const pins = JSON.parse(await readFile(join(here, "pins.json"), "utf8"));
assert.ok(
  !output.startsWith(resolve(here, "../..") + "/"),
  "evidence/consumers must live outside checkout",
);
await mkdir(output, { recursive: true });
assert.deepEqual(await readdir(output), [], "use a fresh empty output directory");
const evidence = {
  outcome: "failed",
  started: new Date().toISOString(),
  host: { platform: platform(), release: release(), arch: arch(), node: process.version },
  pins,
  commands: [],
  cases: [],
};

async function command(executable, args, cwd, label) {
  const record = { executable, args, cwd, label, output: "" };
  evidence.commands.push(record);
  return await new Promise((resolveRun, reject) => {
    const child = spawn(executable, args, {
      cwd,
      detached: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const terminate = (signal) => {
      if (child.pid) {
        try {
          process.kill(-child.pid, signal);
        } catch (error) {
          if (error.code !== "ESRCH") reject(error);
        }
      }
    };
    // Liveness guard only: expiry is a harness failure, never a negative-control pass.
    let killTimer;
    const timer = setTimeout(() => {
      record.timedOut = true;
      terminate("SIGTERM");
      killTimer = setTimeout(() => terminate("SIGKILL"), 5000);
    }, 180_000);
    const capture = (data) => {
      record.output += data;
      process.stdout.write(data);
    };
    child.stdout.on("data", capture);
    child.stderr.on("data", capture);
    child.on("error", (error) => {
      clearTimeout(timer);
      clearTimeout(killTimer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      clearTimeout(killTimer);
      record.code = code;
      record.signal = signal;
      if (record.timedOut) reject(new Error(`${label} exceeded 180-second command liveness guard`));
      else resolveRun(record);
    });
  });
}
function passed(record) {
  assert.equal(record.code, 0, `${record.label}: ${record.output}`);
}
async function put(root, path, text) {
  await mkdir(dirname(join(root, path)), { recursive: true });
  await writeFile(join(root, path), text);
}
function block(doc, marker) {
  const matches = [...doc.matchAll(/^```tsx\r?\n([\s\S]*?)^```/gm)]
    .map((m) => m[1])
    .filter((s) => s.startsWith(`// ${marker}`))
    .filter((s) =>
      marker === "mdx-components.tsx (project root)"
        ? s.includes('import { Island } from "zfb";')
        : marker === "pages/blog/[slug].tsx"
          ? s.includes("pageComponents")
          : true,
    );
  assert.equal(matches.length, 1, `unique release fence: ${marker}`);
  return matches[0];
}
async function consumer(root, version) {
  await mkdir(root, { recursive: true });
  // Direct carrier pin ensures this run cannot silently use an installed workspace binary.
  await put(
    root,
    "package.json",
    JSON.stringify(
      {
        private: true,
        type: "module",
        dependencies: {
          "@takazudo/zfb": version,
          "@takazudo/zfb-runtime": version,
          "@takazudo/zfb-linux-x64-gnu": version,
        },
        devDependencies: { esbuild: "0.25.11", playwright: "1.56.1" },
      },
      null,
      2,
    ),
  );
  passed(
    await command(
      "npm",
      [
        "install",
        "--registry=https://registry.npmjs.org",
        "--include=optional",
        "--no-audit",
        "--no-fund",
      ],
      root,
      `install ${version}`,
    ),
  );
  const lock = JSON.parse(await readFile(join(root, "package-lock.json"), "utf8"));
  for (const [name, expected] of Object.entries(pins.versions[version])) {
    const installed = lock.packages[`node_modules/${name}`];
    assert.equal(installed.version, version);
    assert.equal(installed.integrity, expected.dist.integrity);
    assert.equal(installed.resolved, expected.dist.tarball);
    const packagePath = join(root, "node_modules", name);
    assert.equal(
      await realpath(packagePath),
      packagePath,
      "installed dependency must not be a workspace link",
    );
    assert.equal(
      JSON.parse(await readFile(join(packagePath, "package.json"), "utf8")).version,
      version,
    );
  }
  const binary = join(root, "node_modules/@takazudo/zfb-linux-x64-gnu/zfb");
  evidence.cases.push({
    name: `installed provenance ${version}`,
    root,
    binarySha256: createHash("sha256")
      .update(await readFile(binary))
      .digest("hex"),
    lockfile: join(root, "package-lock.json"),
  });
  const binaryVersion = await command(binary, ["--version"], root, `native version ${version}`);
  passed(binaryVersion);
  assert.match(binaryVersion.output, new RegExp(`\\b${version.replaceAll(".", "\\.")}\\b`));
  return binary;
}
async function staticServer(root) {
  const server = createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
      const file = resolve(root, `.${pathname === "/" ? "/index.html" : pathname}`);
      assert.ok(file.startsWith(`${root}/`));
      res.setHeader(
        "Content-Type",
        file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html",
      );
      res.end(await readFile(file));
    } catch {
      res.writeHead(404);
      res.end();
    }
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return {
    url: `http://127.0.0.1:${server.address().port}/`,
    close: () => new Promise((r) => server.close(r)),
  };
}
async function browserCheck(root, served, verify) {
  let browser;
  const diagnostics = [];
  const row = { diagnostics };
  evidence.cases.push(row);
  try {
    const { chromium } = await import(
      pathToFileURL(join(root, "node_modules/playwright/index.mjs"))
    );
    const executablePath = process.env.CHROMIUM_EXECUTABLE_PATH;
    browser = await chromium.launch({
      headless: true,
      ...(executablePath ? { executablePath } : {}),
    });
    evidence.browser = {
      version: browser.version(),
      executablePath: executablePath ?? chromium.executablePath(),
    };
    const page = await browser.newPage();
    page.on("console", (m) => {
      if (["error", "warning"].includes(m.type()))
        diagnostics.push({ type: m.type(), text: m.text() });
    });
    page.on("pageerror", (e) => diagnostics.push({ type: "pageerror", text: e.message }));
    assert.equal((await page.goto(served.url, { waitUntil: "domcontentloaded" })).status(), 200);
    await verify(page, row);
    assert.deepEqual(diagnostics, [], "unexpected browser diagnostics");
    row.outcome = "passed";
  } finally {
    try {
      await browser?.close();
    } finally {
      await served.close();
    }
  }
}
async function lateShow(version) {
  const root = join(output, `late-show-${version}`);
  await consumer(root, version);
  await copyFile(join(here, "late-show.mjs"), join(root, "late-show.mjs"));
  const { build } = await import(pathToFileURL(join(root, "node_modules/esbuild/lib/main.js")));
  await build({
    absWorkingDir: root,
    entryPoints: ["late-show.mjs"],
    bundle: true,
    format: "esm",
    platform: "browser",
    outfile: join(root, "bundle.js"),
  });
  await put(
    root,
    "index.html",
    '<!doctype html><html><head><link rel="icon" href="data:,"></head><body><script type="module" src="/bundle.js"></script></body></html>',
  );
  await browserCheck(root, await staticServer(root), async (page, row) => {
    row.name = `late Show ${version}`;
    await page.waitForFunction(() => typeof window.verifyLateShow === "function");
    row.results = await page.evaluate(() => window.verifyLateShow());
    for (const result of row.results) {
      assert.equal(result.disposed, true);
      assert.deepEqual(result.diagnostics, []);
      if (version === "3.2.0" && !result.wrapped) {
        assert.ok(result.childNodes > 0, "3.2.0 must reproduce owned DOM leak");
        assert.match(result.after, /<p>later<\/p>/);
      } else {
        assert.equal(result.childNodes, 0, "unmount removes ALL owned DOM");
        assert.equal(result.after, "");
      }
    }
  });
}
async function recipe(version) {
  const root = join(output, `mdx-${version}`);
  const binary = await consumer(root, version);
  const doc = await readFile(join(here, `docs/v${version}.txt`), "utf8");
  assert.equal(createHash("sha256").update(doc).digest("hex"), pins.docs[version].sha256);
  for (const marker of [
    "components/counter.tsx",
    "mdx-components.tsx (project root)",
    "pages/_mdx-components.tsx",
  ]) {
    const source = block(doc, marker);
    assert.match(source, /from "zfb(?:\/zudo-react)?"/);
    assert.doesNotMatch(source, /@takazudo/);
    await put(root, marker.split(" (")[0], source);
  }
  const pageFence = block(doc, "pages/blog/[slug].tsx");
  const imports = pageFence
    .split("\n")
    .filter((s) => s.startsWith("import "))
    .join("\n")
    .replace("../../mdx-components", "../mdx-components")
    .replace("../_mdx-components", "./_mdx-components");
  const call = pageFence.match(/<post\.Content[^]*?\/>/)?.[0];
  assert.ok(call);
  await put(
    root,
    "pages/index.tsx",
    `import { getCollection } from "zfb/content";\n${imports}\nexport default function Page() { const post = getCollection("blog")[0]; return <html><head><link rel="icon" href="data:," /></head><body>${call}</body></html>; }`,
  );
  await put(
    root,
    "content/blog/recipe.mdx",
    "---\ntitle: Recipe\n---\n\n<Counter />\n\n<NamedCounter />\n",
  );
  await put(
    root,
    "zfb.config.json",
    JSON.stringify({ wind: false, collections: [{ name: "blog", path: "content/blog" }] }),
  );
  await put(
    root,
    "tsconfig.json",
    JSON.stringify({ compilerOptions: { jsx: "react-jsx", jsxImportSource: "zfb/zudo-react" } }),
  );
  const built = await command(binary, ["build"], root, `release docs recipe ${version}`);
  if (version === "4.0.0") {
    assert.notEqual(built.code, 0, "4.0.0 bare alias must fail");
    assert.match(built.output, /zfb\/zudo-react/);
    assert.match(built.output, /resolve|not found|unable|Could not/i);
    evidence.cases.push({
      name: "4.0.0 bare alias negative control",
      outcome: "passed",
      build: built,
    });
    return;
  }
  passed(built);
  await browserCheck(root, await staticServer(join(root, "dist")), async (page, row) => {
    row.name = "4.1.0 release Fragment MDX recipe";
    await page.waitForFunction(() => {
      const nodes = [...document.querySelectorAll("[data-zfb-island]")];
      return nodes.length === 2 && nodes.every((n) => n.hasAttribute("data-zfb-island-mounted"));
    });
    row.registration = await page
      .locator("[data-zfb-island]")
      .evaluateAll((ns) => ns.map((n) => n.getAttribute("data-zfb-island")).sort());
    assert.deepEqual(row.registration, ["Counter", "NamedCounter"]);
    const buttons = page.locator("button");
    assert.equal(await buttons.count(), 2);
    assert.equal((await buttons.nth(0).textContent()).trim(), "Count: 0");
    assert.equal((await buttons.nth(1).textContent()).trim(), "Named counter: 0");
    await buttons.nth(0).click();
    await buttons.nth(1).click();
    await page.waitForFunction(() => {
      const b = [...document.querySelectorAll("button")];
      return (
        b[0]?.textContent.trim() === "Count: 1" && b[1]?.textContent.trim() === "Named counter: 1"
      );
    });
    row.clicked = ["Count: 1", "Named counter: 1"];
  });
  // Exact recipe single-variable rejection control: remove only Fragment delimiters.
  const mapPath = join(root, "mdx-components.tsx");
  const original = await readFile(mapPath, "utf8");
  assert.match(original, /<>/);
  await writeFile(mapPath, original.replace(/\s*<>\s*/, "\n").replace(/\s*<\/>\s*/, "\n"));
  const rejected = await command(binary, ["build"], root, "4.1.0 non-Fragment opaque map control");
  assert.notEqual(rejected.code, 0, "non-Fragment control must stay rejected");
  assert.match(rejected.output, /boundary wrapper escapes into an opaque container/i);
  evidence.cases.push({
    name: "4.1.0 non-Fragment opaque map",
    outcome: "passed",
    build: rejected,
  });
}
try {
  await lateShow("4.1.0");
  await lateShow("3.2.0");
  await recipe("4.1.0");
  await recipe("4.0.0");
  evidence.outcome = "passed";
} catch (error) {
  evidence.error = error.stack;
  process.exitCode = 1;
  console.error(error);
} finally {
  await writeFile(join(output, "evidence.json"), JSON.stringify(evidence, null, 2) + "\n");
}
