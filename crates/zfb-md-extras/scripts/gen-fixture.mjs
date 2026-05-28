#!/usr/bin/env node
/**
 * gen-fixture.mjs — one-time fixture generator for zfb-md-extras snapshot tests.
 *
 * Usage:
 *   node gen-fixture.mjs --plugin <npm-pkg> --input <input.md> --output <expected.html>
 *
 * The script:
 *   1. Reads the markdown source from --input.
 *   2. Invokes the upstream remark/rehype plugin by requiring it via
 *      pnpm dlx (so no permanent install is needed).
 *   3. Writes the produced HTML to --output.
 *
 * Workflow integration:
 *   Run once when porting a new plugin, then commit both input.md and
 *   expected.html. Subsequent CI runs use the committed expected.html — they
 *   never call this script.
 *
 * Plugin contract:
 *   The npm package at --plugin must export (or re-export) a function
 *   compatible with the following interface:
 *
 *     export default function plugin(markdown: string): string
 *
 *   or, for unified/remark/rehype-style plugins, export a unified processor
 *   factory (see "remark-style" mode below).
 *
 *   Wave 2 (#569) will pin the exact plugin interface once the Pipeline
 *   constructor exists. For now, two calling modes are supported:
 *
 *   1. **Direct-function mode** (default): the module default export is a
 *      `(markdown: string) => string` function.
 *   2. **Remark-style mode** (`--remark`): the module default export is a
 *      remark plugin. The script builds a minimal
 *      `remark().use(plugin).process(input)` pipeline via `pnpm dlx remark`.
 *
 * Examples:
 *   # Direct-function mode (trivial plugin for smoke-testing):
 *   node gen-fixture.mjs --plugin ./local-plugin.mjs \
 *     --input tests/fixtures/basic/input.md \
 *     --output tests/fixtures/basic/expected.html
 *
 *   # Remark-style mode:
 *   node gen-fixture.mjs --plugin remark-gfm --remark \
 *     --input tests/fixtures/gfm/input.md \
 *     --output tests/fixtures/gfm/expected.html
 */

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

// ── CLI parsing ──────────────────────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    plugin: { type: "string" },
    input: { type: "string" },
    output: { type: "string" },
    remark: { type: "boolean", default: false },
    help: { type: "boolean", default: false, short: "h" },
  },
  strict: true,
});

if (values.help) {
  console.log(
    `
Usage: node gen-fixture.mjs --plugin <npm-pkg|./local.mjs> --input <input.md> --output <expected.html> [--remark]

Options:
  --plugin   npm package name or local path to the plugin module
  --input    path to the markdown input file
  --output   path to write the generated HTML (created if needed)
  --remark   use remark-style unified pipeline mode (default: direct-function mode)
  -h/--help  show this message
`.trim(),
  );
  process.exit(0);
}

if (!values.plugin || !values.input || !values.output) {
  console.error("Error: --plugin, --input, and --output are all required.");
  console.error("Run with --help for usage.");
  process.exit(1);
}

// ── Read input ───────────────────────────────────────────────────────────────

const inputPath = resolve(values.input);
const outputPath = resolve(values.output);
const markdown = readFileSync(inputPath, "utf8");

// ── Transform ────────────────────────────────────────────────────────────────

let html;

if (values.remark) {
  // Remark-style mode placeholder.
  //
  // NOTE: Most remark/rehype plugins (e.g. remark-gfm) are not CLI executables
  // and cannot be invoked via `pnpm dlx <plugin>`. This mode is a placeholder
  // for Wave 2 (#569), which will wire the real unified/remark pipeline once
  // the Pipeline constructor and plugin interface are stabilised.
  //
  // For now, --remark mode expects the named package to expose a CLI binary
  // that accepts Markdown on stdin and emits HTML on stdout. If your plugin
  // does not do this, use a small wrapper script (e.g. `./run-plugin.mjs`)
  // in direct-function mode instead.
  console.error(`[gen-fixture] remark mode: invoking pnpm dlx ${values.plugin}`);
  console.error(
    "[gen-fixture] NOTE: --remark requires the plugin package to expose a CLI that reads stdin/writes stdout.",
  );
  const cmd = `pnpm dlx ${values.plugin}`;
  html = execSync(cmd, {
    input: markdown,
    encoding: "utf8",
    stdio: ["pipe", "pipe", "inherit"],
  });
} else {
  // Direct-function mode: import the module and call its default export.
  // Supports both local paths and installed packages.
  let pluginPath = values.plugin;
  if (!pluginPath.startsWith(".") && !pluginPath.startsWith("/")) {
    // npm package name: resolve via node_modules or pnpm dlx on-demand.
    // For now, assume the package is available in the local node_modules tree
    // (the caller must install it first, or run via `pnpm dlx`).
    pluginPath = values.plugin;
  } else {
    pluginPath = resolve(values.plugin);
  }

  console.error(`[gen-fixture] direct mode: importing ${pluginPath}`);

  // Dynamic import — supports both ESM and CommonJS (via createRequire).
  let mod;
  try {
    mod = await import(pluginPath);
  } catch (err) {
    console.error(`[gen-fixture] Failed to import ${pluginPath}: ${err.message}`);
    console.error("Hint: install the plugin first (pnpm add -D <plugin>) or use --remark mode.");
    process.exit(1);
  }

  const fn_ = mod.default ?? mod;
  if (typeof fn_ !== "function") {
    console.error(`[gen-fixture] plugin default export is not a function (got ${typeof fn_}).`);
    process.exit(1);
  }

  html = fn_(markdown);
  if (html instanceof Promise) {
    html = await html;
  }
}

// ── Write output ─────────────────────────────────────────────────────────────

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, html, "utf8");
console.error(`[gen-fixture] wrote ${outputPath}`);
console.log(html);
