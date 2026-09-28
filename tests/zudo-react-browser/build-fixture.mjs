/**
 * Build the packed-SDK server/browser fixture pages used by this harness.
 *
 * Scenario contract for #3280: each directory in fixtures/ contains a
 * scenario.json with `page`, `component`, `props`, and `mode` (`hydrate`,
 * `mount`, or `none`), plus a TSX module whose default export is the component.
 * An optional `run()` export may publish scenario-specific observations. The
 * generated bootstrap stores `{ root, flush, result, mode, identity }` on
 * `globalThis.__zudoReactBrowser`; its URL under /generated/ can be held with
 * `page.route` to inspect SSR DOM before the module executes.
 */

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HARNESS_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HARNESS_DIR, "../..");
const FIXTURES_DIR = join(HARNESS_DIR, "fixtures");
const GENERATED_DIR = join(HARNESS_DIR, "dist");
const STAGED_PACKAGE_DIR = join(HARNESS_DIR, "node_modules", "@takazudo", "zfb");
const TSC_CONFIG = join(FIXTURES_DIR, "tsconfig.json");
const PACKED_SDK_SCRIPT = join(REPO_ROOT, "packages/zfb/scripts/zudo-react-packed.mjs");
const IMPORT_MAP_PREFIX = "/zfb-dist/";
const BUILD_ID = "zudo-react-browser-harness-v1";
const MODES = new Set(["hydrate", "mount", "none"]);

function run(binary, args, cwd) {
  execFileSync(binary, args, { cwd, stdio: "inherit" });
}

function assertInside(root, target, label) {
  const path = relative(root, target);
  if (path === "" || path === ".." || path.startsWith(`..${sep}`)) {
    throw new Error(`${label} escapes its fixture directory: ${target}`);
  }
}

function inlineJson(value) {
  return JSON.stringify(value)
    .replaceAll("&", "\\u0026")
    .replaceAll("<", "\\u003c")
    .replaceAll(">", "\\u003e")
    .replaceAll("\u2028", "\\u2028")
    .replaceAll("\u2029", "\\u2029");
}

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function readScenarios() {
  return readdirSync(FIXTURES_DIR, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((entry) => {
      const directory = join(FIXTURES_DIR, entry.name);
      const configPath = join(directory, "scenario.json");
      const config = JSON.parse(readFileSync(configPath, "utf8"));
      if (typeof config.page !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(config.page)) {
        throw new Error(`Invalid page name in ${configPath}`);
      }
      if (typeof config.component !== "string" || extname(config.component) !== ".tsx") {
        throw new Error(`Scenario ${config.page} must name a .tsx component`);
      }
      if (!MODES.has(config.mode)) {
        throw new Error(`Scenario ${config.page} has unsupported mode: ${config.mode}`);
      }
      if (
        config.props === null ||
        typeof config.props !== "object" ||
        Array.isArray(config.props)
      ) {
        throw new Error(`Scenario ${config.page} props must be an object`);
      }
      const componentPath = resolve(directory, config.component);
      assertInside(directory, componentPath, `Scenario ${config.page} component`);
      if (!statSync(componentPath).isFile()) {
        throw new Error(`Scenario ${config.page} component is not a file: ${componentPath}`);
      }
      return { config, directory, componentPath };
    });
}

function createImportMap(stagedPackage) {
  const exportsMap = stagedPackage.exports;
  if (!exportsMap || typeof exportsMap !== "object") {
    throw new Error("The staged SDK package has no exports map");
  }

  const runtimeExports = Object.entries(exportsMap).filter(([subpath]) =>
    /^\.\/zudo-react(?:\/.*)?$/.test(subpath),
  );
  if (runtimeExports.length !== 5) {
    throw new Error(
      `Expected five zudo-react exports in staged package, found ${runtimeExports.length}`,
    );
  }

  const imports = {};
  for (const [subpath, conditions] of runtimeExports) {
    const target = typeof conditions === "string" ? conditions : conditions?.default;
    if (typeof target !== "string" || !target.startsWith("./dist/")) {
      throw new Error(`Staged export ${subpath} does not point into dist/: ${target}`);
    }
    imports[`@takazudo/zfb${subpath.slice(1)}`] =
      IMPORT_MAP_PREFIX + target.slice("./dist/".length);
  }
  return imports;
}

function createBootstrap({ config, identity, componentUrl, containerSelector }) {
  return `import * as scenario from ${inlineJson(componentUrl)};
import { flush, h } from "@takazudo/zfb/zudo-react";

const component = scenario.default;
const props = ${inlineJson(config.props)};
const mode = ${inlineJson(config.mode)};
const identity = ${inlineJson(identity)};
const container = document.querySelector(${inlineJson(containerSelector)});
if (!(container instanceof Element)) throw new Error("Generated scenario root was not found");

let root = null;
if (mode === "hydrate" || mode === "mount") {
  const client = await import("@takazudo/zfb/zudo-react/client");
  root = client[mode](h(component, props), container, { identity });
}
await flush();
const result = typeof scenario.run === "function" ? await scenario.run() : undefined;
globalThis.__zudoReactBrowser = { root, flush, result, mode, identity };
`;
}

async function main() {
  rmSync(GENERATED_DIR, { recursive: true, force: true });
  rmSync(STAGED_PACKAGE_DIR, { recursive: true, force: true });
  mkdirSync(GENERATED_DIR, { recursive: true });

  run(process.execPath, [PACKED_SDK_SCRIPT, "stage", HARNESS_DIR], REPO_ROOT);
  run("pnpm", ["--filter", "@takazudo/zfb", "exec", "tsc", "-p", TSC_CONFIG], REPO_ROOT);

  const stagedPackage = JSON.parse(readFileSync(join(STAGED_PACKAGE_DIR, "package.json"), "utf8"));
  const importMap = createImportMap(stagedPackage);
  const { h } = await import("@takazudo/zfb/zudo-react");
  const { islandRoot, renderToString } = await import("@takazudo/zfb/zudo-react/server");
  const scenarios = readScenarios();
  if (scenarios.length === 0) throw new Error("No scenario directories found");
  const pageNames = new Set();

  for (const { config, componentPath } of scenarios) {
    if (pageNames.has(config.page)) throw new Error(`Duplicate scenario page name: ${config.page}`);
    pageNames.add(config.page);

    const compiledRelative = relative(FIXTURES_DIR, componentPath).replace(/\.tsx$/, ".js");
    const compiledPath = join(GENERATED_DIR, "compiled", compiledRelative);
    const componentModule = await import(pathToFileURL(compiledPath).href);
    const component = componentModule.default;
    if (typeof component !== "function" || !component.name) {
      throw new Error(`Scenario ${config.page} must default-export a named component`);
    }

    const identity = { component: component.name, build: BUILD_ID };
    const serverTree = islandRoot(h(component, config.props), { identity });
    const serverHtml = renderToString(serverTree);
    const componentUrl = `/compiled/${compiledRelative
      .split(sep)
      .map(encodeURIComponent)
      .join("/")}`;
    const bootstrapPath = join(GENERATED_DIR, "generated", `${config.page}.js`);
    mkdirSync(dirname(bootstrapPath), { recursive: true });
    writeFileSync(
      bootstrapPath,
      createBootstrap({
        config,
        identity,
        componentUrl,
        containerSelector: "#scenario-root > [data-zfb-island]",
      }),
    );

    const pageHtml = `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>${escapeHtml(config.page)}</title>
    <script type="importmap">${inlineJson({ imports: importMap })}</script>
  </head>
  <body>
    <main id="scenario-root">${serverHtml}</main>
    <script type="module" src="/generated/${encodeURIComponent(config.page)}.js"></script>
  </body>
</html>
`;
    writeFileSync(join(GENERATED_DIR, `${config.page}.html`), pageHtml);
    console.log(`fixture generated: ${config.page} (${config.mode})`);
  }

  const defaultPage = scenarios.find(({ config }) => config.mode === "none") ?? scenarios[0];
  writeFileSync(
    join(GENERATED_DIR, "index.html"),
    `<!doctype html><meta http-equiv="refresh" content="0; url=/${encodeURIComponent(defaultPage.config.page)}.html">\n`,
  );
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
