#!/usr/bin/env node
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const slug = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const hash = (value) => createHash("sha256").update(value).digest("hex");
const json = (value) => `${JSON.stringify(value, null, 2)}\n`;
const sorted = (values) => [...new Set(values)].sort();
const fail = (message) => {
  throw new Error(message);
};

export function mergeConfig(base, override = {}) {
  const result = structuredClone(base);
  for (const [key, value] of Object.entries(override)) {
    if (value === null) delete result[key];
    else if (typeof value === "object" && !Array.isArray(value))
      result[key] = mergeConfig(result[key] ?? {}, value);
    else result[key] = value;
  }
  return result;
}

/** @typedef {import("../wind-examples/types.js").WindExampleFamily} WindExampleFamily */

// This interface deliberately accepts literal HTML only. Dynamic class generation
// or alternate class attributes would break the displayed/compiled source contract.
export function validateRecord(record) {
  if (
    record.schemaVersion !== 1 ||
    !slug.test(record.family) ||
    !Array.isArray(record.examples) ||
    !record.examples.length
  )
    fail("Invalid family record");
  const ids = new Set();
  for (const example of record.examples) {
    const label = `${record.family}/${example.id}`;
    if (!slug.test(example.id) || ids.has(example.id)) fail(`${label}: invalid or duplicate id`);
    ids.add(example.id);
    if (!["positive", "expected-diagnostic"].includes(example.kind))
      fail(`${label}: explicit sample kind required`);
    for (const key of ["utilities", "authoredClasses"]) {
      if (
        !Array.isArray(example[key]) ||
        example[key].some((name) => typeof name !== "string" || !name || /\s/.test(name)) ||
        sorted(example[key]).length !== example[key].length
      )
        fail(`${label}: invalid ${key}`);
    }
    if (typeof example.html !== "string" || typeof example.scaffoldCss !== "string")
      fail(`${label}: html and scaffoldCss required`);
    if (
      example.head !== undefined &&
      (example.kind !== "positive" || example.head !== '<base href="about:srcdoc">')
    )
      fail(`${label}: only positive examples may declare the about:srcdoc fragment base`);
    if (
      /\bclassName\s*=|\bclass\s*=\s*[^"']|\bclass\s*=\s*['"][^'"]*[&{}]|<script\b|<style\b|<link\b/i.test(
        example.html,
      )
    )
      fail(`${label}: only static quoted class attributes; no embedded styles or scripts`);
    const classes = sorted(
      [...example.html.matchAll(/\bclass\s*=\s*(?:"([^"]*)"|'([^']*)')/gi)].flatMap((match) =>
        (match[1] ?? match[2]).split(/\s+/).filter(Boolean),
      ),
    );
    const declared = sorted([...example.utilities, ...example.authoredClasses]);
    if (json(classes) !== json(declared))
      fail(`${label}: every markup class must have exactly one declared utility/scaffold intent`);
    if (example.utilities.some((name) => example.authoredClasses.includes(name)))
      fail(`${label}: overlapping utility/scaffold intent`);
    if (example.authoredClasses.some((name) => !/^wind-demo(?:-[a-z0-9]+)*$/.test(name)))
      fail(`${label}: scaffold classes must use wind-demo namespace`);
    if (
      /@(?:import|tailwind|apply|theme|source|utility|plugin|config|reference)\b/i.test(
        example.scaffoldCss,
      )
    )
      fail(`${label}: standalone authored CSS only`);
    // No handwritten selector may emulate a declared utility.
    const scaffoldSelectors = [...example.scaffoldCss.matchAll(/\.([a-zA-Z_][\w-]*)/g)].map(
      (match) => match[1],
    );
    if (scaffoldSelectors.some((name) => !example.authoredClasses.includes(name)))
      fail(`${label}: scaffold selector must be declared`);
    if (
      example.config &&
      (typeof example.config !== "object" ||
        Object.keys(example.config).some((key) => key !== "wind"))
    )
      fail(`${label}: only wind configuration overrides are supported`);
    if (
      example.config?.wind === false ||
      (example.kind === "positive" && example.config?.wind?.strict === false) ||
      example.config?.wind?.safelist ||
      example.config?.wind?.authoredClasses ||
      example.config?.wind?.sources ||
      example.config?.wind?.manifests
    )
      fail(`${label}: config cannot override candidate intent or strictness`);
    if (
      example.candidateOrigin !== undefined &&
      (example.kind === "positive" || !["source", "safelist"].includes(example.candidateOrigin))
    )
      fail(`${label}: only diagnostic fixtures may select source/safelist origin`);
    const expected = example.expectedDiagnostics ?? [];
    if (
      (example.kind === "positive" && expected.length) ||
      (example.kind === "expected-diagnostic" && !expected.length)
    )
      fail(`${label}: diagnostics do not match sample kind`);
    if (
      expected.some(
        (item) => !/^ZW\d{3}$/.test(item.code) || !["error", "warning"].includes(item.severity),
      ) ||
      sorted(expected.map((item) => `${item.severity}:${item.code}`)).length !== expected.length
    )
      fail(`${label}: invalid expected diagnostics`);
  }
  return record;
}

/** @returns {WindExampleFamily[]} */
export function loadRecords(root = REPO_ROOT) {
  const source = join(root, "docs/wind-examples");
  return readdirSync(source)
    .filter(
      (name) => name.endsWith(".json") && !["base-config.json", "compiler.json"].includes(name),
    )
    .sort()
    .map((name) => {
      const record = validateRecord(JSON.parse(readFileSync(join(source, name), "utf8")));
      if (name !== `${record.family}.json`) fail(`${name}: filename must match family`);
      return record;
    });
}

/** Exact bytes used for displayed source, iframe markup and compiler input. */
export function exampleSource(example) {
  return `${example.html}\n`;
}

export function exampleAssetPath(family, id, extension = "css") {
  if (!slug.test(family) || !slug.test(id) || !["css", "html"].includes(extension))
    fail("Invalid asset identity");
  return `wind-examples/${family}/${id}.${extension}`;
}

// Relative paths are base-neutral. Consumers prepend the configured site base;
// HtmlPreview's isolated iframe receives the CSS bytes and the SAME record HTML.
export function assetUrl(path, base = "/") {
  if (
    !/^wind-examples\/[a-z0-9-]+\/[a-z0-9-]+\.(css|html)$/.test(path) ||
    !base.startsWith("/") ||
    /[?#]/.test(base)
  )
    fail("Invalid base or asset path");
  return `${base.replace(/\/$/, "")}/${path}`;
}

function walk(path) {
  return readdirSync(path, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name, "en"))
    .flatMap((entry) =>
      entry.isDirectory()
        ? ["node_modules", "target", "build", "dist", ".git"].includes(entry.name)
          ? []
          : walk(join(path, entry.name))
        : [join(path, entry.name)],
    );
}
export function compilerSourceDigest(root) {
  const paths = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ...walk(join(root, "crates"))
      .filter((path) => /\.(rs|toml|json|css)$/.test(path))
      .map((path) => relative(root, path)),
  ].sort();
  return hash(paths.map((path) => `${path}\0${hash(readFileSync(join(root, path)))}\n`).join(""));
}

export function assertDiagnostics(example, result) {
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`.replace(/\u001b\[[0-9;]*m/g, "");
  if (result.error || result.signal || result.status === null)
    fail(`Compiler process failed: ${result.error ?? result.signal}`);
  const codes = sorted([...output.matchAll(/\bZW\d{3}\b/g)].map(([code]) => code));
  if (example.kind === "positive") {
    if (result.status !== 0 || codes.length)
      fail(`${example.id}: positive sample failed or reported diagnostics\n${output}`);
  } else {
    const expected = sorted(example.expectedDiagnostics.map((item) => item.code));
    const hasError = example.expectedDiagnostics.some((item) => item.severity === "error");
    // zfb css reports fatal diagnostics as a failed process, and warnings on a
    // successful process. A fixture cannot mix severities: fatal emission hides warnings.
    if (example.expectedDiagnostics.some((item) => (item.severity === "error") !== hasError))
      fail(`${example.id}: mixed-severity diagnostic fixture is unsupported`);
    if ((result.status !== 0) !== hasError || json(codes) !== json(expected))
      fail(`${example.id}: expected diagnostic/severity mismatch\n${output}`);
  }
}

export function assertFresh(outputDir, assets) {
  const actual = existsSync(outputDir)
    ? walk(outputDir)
        .map((path) => relative(outputDir, path))
        .sort()
    : [];
  const expected = [...assets.keys()].sort();
  if (json(actual) !== json(expected))
    fail("Stale wind preview asset inventory; regenerate assets");
  for (const [path, bytes] of assets)
    if (readFileSync(join(outputDir, path), "utf8") !== bytes)
      fail(`Stale wind preview asset: ${path}; regenerate assets`);
}

export function generateAssets({ root = REPO_ROOT, compiler, check = false, run = spawnSync }) {
  if (!compiler || !isAbsolute(compiler))
    fail(
      "An explicit absolute workspace --compiler path is required; PATH and docs' published zfb are never used",
    );
  const source = join(root, "docs/wind-examples");
  const pin = JSON.parse(readFileSync(join(source, "compiler.json"), "utf8"));
  const version = run(compiler, ["-V"], { encoding: "utf8" });
  if (version.status !== 0 || version.stdout.trim() !== `zfb ${pin.version}`)
    fail(`Expected workspace zfb ${pin.version}, got ${version.stdout ?? version.error}`);
  const base = JSON.parse(readFileSync(join(source, "base-config.json"), "utf8"));
  if (base.wind?.strict !== true || base.wind?.reset !== "none")
    fail("Base config must explicitly use strict wind and reset:none");
  const records = loadRecords(root);
  const workspace = mkdtempSync(join(tmpdir(), "zfb-wind-preview-"));
  const assets = new Map();
  const manifest = {
    schemaVersion: 1,
    compiler: { version: pin.version, sourceSha256: compilerSourceDigest(root) },
    examples: [],
  };
  try {
    for (const record of records)
      for (const example of record.examples) {
        const config = mergeConfig(base, example.config);
        if (example.kind === "positive") config.wind.strict = true;
        config.wind.authoredClasses = Object.fromEntries(
          example.authoredClasses.map((name) => [name, true]),
        );
        const candidateOrigin =
          example.kind === "positive" ? "safelist" : (example.candidateOrigin ?? "safelist");
        config.wind.safelist =
          candidateOrigin === "safelist" ? { "docs-preview": example.utilities } : {};
        const html = exampleSource(example);
        const input = `${example.scaffoldCss.trimEnd()}\n`;
        writeFileSync(join(workspace, "sample.html"), html);
        writeFileSync(join(workspace, "input.css"), input);
        writeFileSync(join(workspace, "zfb.config.json"), json(config));
        const output = join(workspace, "compiled.css");
        rmSync(output, { force: true });
        const args = [
          "css",
          "--input",
          join(workspace, "input.css"),
          "--output",
          output,
          "--config",
          join(workspace, "zfb.config.json"),
          "--project-root",
          workspace,
          "--source",
          "sample.html",
          "--no-auto-source",
          "--no-default-highlight-styles",
        ];
        const result = run(compiler, args, {
          cwd: root,
          encoding: "utf8",
          env: { ...process.env, NO_COLOR: "1" },
        });
        assertDiagnostics(example, result);
        const entry = {
          family: record.family,
          id: example.id,
          kind: example.kind,
          candidateOrigin,
          htmlSha256: hash(html),
          headSha256: hash(example.head ?? ""),
          inputSha256: hash(input),
          configSha256: hash(json(config)),
          expectedDiagnostics: example.expectedDiagnostics ?? [],
        };
        if (example.kind === "positive") {
          const css = `${readFileSync(output, "utf8").trimEnd()}\n`;
          if (/@layer\s+(?:theme|base)|tailwindcss|--tw-|@import\b/.test(css))
            fail(`${example.id}: parent/Tailwind CSS contamination`);
          entry.cssPath = exampleAssetPath(record.family, example.id);
          entry.htmlPath = exampleAssetPath(record.family, example.id, "html");
          entry.cssSha256 = hash(css);
          assets.set(entry.cssPath.replace(/^wind-examples\//, ""), css);
          assets.set(entry.htmlPath.replace(/^wind-examples\//, ""), html);
        }
        manifest.examples.push(entry);
      }
    assets.set("manifest.json", json(manifest));
    const outputDir = join(root, "docs/public/wind-examples");
    if (check) assertFresh(outputDir, assets);
    else {
      rmSync(outputDir, { recursive: true, force: true });
      for (const [path, bytes] of assets) {
        mkdirSync(dirname(join(outputDir, path)), { recursive: true });
        writeFileSync(join(outputDir, path), bytes);
      }
    }
    return manifest;
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    const index = args.indexOf("--compiler");
    if (
      index < 0 ||
      !args[index + 1] ||
      args.some((value, i) => i !== index + 1 && !["--compiler", "--check"].includes(value))
    )
      fail(
        "Usage: node docs/scripts/wind-preview-assets.mjs --compiler /absolute/path/to/workspace/zfb [--check]",
      );
    const manifest = generateAssets({ compiler: args[index + 1], check: args.includes("--check") });
    console.log(
      `Wind previews ${args.includes("--check") ? "fresh" : "regenerated"}: ${manifest.examples.length} samples (including diagnostic fixtures)`,
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
