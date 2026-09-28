/**
 * Build the packed-SDK server/browser fixture pages used by this harness.
 *
 * Scenario contract for #3280: each directory in fixtures/ contains a
 * scenario.json with `page` and either the legacy single-root fields
 * (`component`, `props`, `mode`) or a `roots` array. Root entries may point at
 * different TSX modules and may request a pre-commit abort or a deliberate
 * post-SSR mutation. An optional fixture `page.css` can be loaded with the
 * scenario's `stylesheet` field. The generated bootstrap stores `{ roots,
 * root, flush, result, mode, identity, client, h }` on
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
      const roots = config.roots ?? [
        { component: config.component, props: config.props, mode: config.mode },
      ];
      if (!Array.isArray(roots) || roots.length === 0)
        throw new Error(`Scenario ${config.page} must define at least one root`);
      const normalizedRoots = roots.map((root, index) => {
        if (typeof root.component !== "string" || extname(root.component) !== ".tsx") {
          throw new Error(`Scenario ${config.page} root ${index} must name a .tsx component`);
        }
        if (!MODES.has(root.mode)) {
          throw new Error(
            `Scenario ${config.page} root ${index} has unsupported mode: ${root.mode}`,
          );
        }
        if (root.props === null || typeof root.props !== "object" || Array.isArray(root.props)) {
          throw new Error(`Scenario ${config.page} root ${index} props must be an object`);
        }
        const componentPath = resolve(directory, root.component);
        assertInside(directory, componentPath, `Scenario ${config.page} root ${index} component`);
        if (!statSync(componentPath).isFile()) {
          throw new Error(`Scenario ${config.page} component is not a file: ${componentPath}`);
        }
        return { ...root, props: root.props, componentPath, index };
      });
      let stylesheetPath;
      if (config.stylesheet !== undefined) {
        if (typeof config.stylesheet !== "string")
          throw new Error(`Scenario ${config.page} stylesheet must be a path`);
        stylesheetPath = resolve(directory, config.stylesheet);
        assertInside(directory, stylesheetPath, `Scenario ${config.page} stylesheet`);
        if (!statSync(stylesheetPath).isFile())
          throw new Error(`Scenario ${config.page} stylesheet is not a file: ${stylesheetPath}`);
      }
      return { config, directory, roots: normalizedRoots, stylesheetPath };
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

function createBootstrap(roots) {
  const imports = roots
    .map(
      ({ componentUrl }, index) => `import * as scenario${index} from ${inlineJson(componentUrl)};`,
    )
    .join("\n");
  const entries = roots
    .map(({ config, identity, mode, index, abortDuringSetup, useTransportProps }) => {
      const selector = `#scenario-root > [data-zudo-browser-root="root-${index}"]`;
      return `{
  const scenario = scenario${index};
  const component = scenario.default;
  const serverProps = ${inlineJson(config.props)};
  const mode = ${inlineJson(mode)};
  const identity = ${inlineJson(identity)};
  const container = document.querySelector(${inlineJson(selector)});
  if (!(container instanceof Element)) throw new Error("Generated scenario root ${index} was not found");
  const diagnostics = [];
  let transportError;
  const options = { identity, report: (diagnostic) => diagnostics.push(diagnostic) };
  ${abortDuringSetup ? "const abortController = new AbortController(); options.signal = abortController.signal; window.__zudoReactAbortDuringSetup = () => abortController.abort();" : ""}
  let props = serverProps;
  ${useTransportProps ? "try { props = client.parseProps(container.getAttribute('data-props') ?? ''); } catch (error) { transportError = String(error); }" : ""}
  const node = h(component, props);
  const root = !transportError && (mode === "hydrate" || mode === "mount") ? client[mode](node, container, options) : null;
  roots.push({ index: ${index}, root, node, container, identity, mode, diagnostics, scenario, transportError });
}`;
    })
    .join("\n");
  return `${imports}
import { flush, h } from "@takazudo/zfb/zudo-react";

const modes = ${inlineJson(roots.map(({ mode }) => mode))};
const client = modes.some((mode) => mode === "hydrate" || mode === "mount")
  ? await import("@takazudo/zfb/zudo-react/client")
  : null;
const roots = [];
${entries}
await flush();
for (const entry of roots) entry.result = typeof entry.scenario.run === "function"
  ? await entry.scenario.run(entry.index)
  : undefined;
const first = roots[0];
globalThis.__zudoReactBrowser = {
  roots,
  root: first?.root ?? null,
  flush,
  result: first?.result,
  mode: first?.mode,
  identity: first?.identity,
  client,
  h,
};
`;
}

function mutateServerHtml(html, mutation) {
  if (!mutation) return html;
  switch (mutation.type) {
    case "wrong-tag":
      return html
        .replace(`<${mutation.from}>`, `<${mutation.to}>`)
        .replace(`</${mutation.from}>`, `</${mutation.to}>`);
    case "remove-region-marker":
      return html.replace(/<!--zr:1:\d+:[^>]+-->/, "");
    case "wrong-text":
      return html.replace(mutation.from, mutation.to);
    case "identity-attribute": {
      const name = mutation.name;
      const expression = new RegExp(`${name}="[^"]*"`);
      return html.replace(expression, `${name}="${mutation.value}"`);
    }
    case "malformed-props":
      return html.replace(/data-props="[^"]*"/, 'data-props="{"');
    default:
      throw new Error(`Unsupported server HTML mutation: ${mutation.type}`);
  }
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

  for (const { config, roots, stylesheetPath } of scenarios) {
    if (pageNames.has(config.page)) throw new Error(`Duplicate scenario page name: ${config.page}`);
    pageNames.add(config.page);

    const generatedRoots = [];
    const serverHtml = roots.map(async (root) => {
      const compiledRelative = relative(FIXTURES_DIR, root.componentPath).replace(/\.tsx$/, ".js");
      const compiledPath = join(GENERATED_DIR, "compiled", compiledRelative);
      const componentModule = await import(pathToFileURL(compiledPath).href);
      const component = componentModule.default;
      if (typeof component !== "function" || !component.name) {
        throw new Error(
          `Scenario ${config.page} root ${root.index} must default-export a named component`,
        );
      }

      const identity = { component: component.name, build: BUILD_ID };
      const serverTree = islandRoot(h(component, root.props), { identity });
      let html = renderToString(serverTree);
      html = html.replace(/^<div /, `<div data-zudo-browser-root="root-${root.index}" `);
      html = mutateServerHtml(html, root.mutate);
      const componentUrl = `/compiled/${compiledRelative
        .split(sep)
        .map(encodeURIComponent)
        .join("/")}`;
      generatedRoots[root.index] = {
        config: root,
        identity,
        mode: root.mode,
        index: root.index,
        componentUrl,
        abortDuringSetup: root.abortDuringSetup === true,
        useTransportProps: root.useTransportProps === true,
      };
      return html;
    });
    const renderedRootHtml = (await Promise.all(serverHtml)).join("");
    const bootstrapPath = join(GENERATED_DIR, "generated", `${config.page}.js`);
    mkdirSync(dirname(bootstrapPath), { recursive: true });
    writeFileSync(bootstrapPath, createBootstrap(generatedRoots));

    const style = stylesheetPath
      ? `<link rel="stylesheet" href="/fixtures/${encodeURIComponent(config.page)}/${encodeURIComponent(config.stylesheet)}">`
      : "";

    const pageHtml = `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>${escapeHtml(config.page)}</title>
    ${style}
    <script type="importmap">${inlineJson({ imports: importMap })}</script>
  </head>
  <body>
    <main id="scenario-root">${renderedRootHtml}</main>
    <script type="module" src="/generated/${encodeURIComponent(config.page)}.js"></script>
  </body>
</html>
`;
    writeFileSync(join(GENERATED_DIR, `${config.page}.html`), pageHtml);
    console.log(`fixture generated: ${config.page} (${roots.map((root) => root.mode).join(", ")})`);
    if (stylesheetPath) {
      const outputStylesheet = join(GENERATED_DIR, "fixtures", config.page, config.stylesheet);
      mkdirSync(dirname(outputStylesheet), { recursive: true });
      writeFileSync(outputStylesheet, readFileSync(stylesheetPath));
    }
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
