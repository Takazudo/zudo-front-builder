#!/usr/bin/env node
// PROTOTYPE Node benchmark (zfb#1855, epic zfb#1854): `parseToAst` +
// `JSON.parse` (through the REAL built package — the round trip IS the
// product cost) vs `remark-parse` under capability-matched configs.
// NOT wired into `pnpm test`; run explicitly:
//
//   pnpm --filter @takazudo/zfb-md-wasm build   # once, real dist/ required
//   node crates/zfb-md-wasm/npm/test/bench/bench-parse-ast.mjs
//
// Methodology (the honesty requirements — do not quietly change these):
// - CAPABILITY PARITY: both sides parse MDX + full GFM. Rust side:
//   `pipeline.gfm` all-on (strikethrough, table, autolinkLiteral,
//   taskListItem, footnoteDefinition — remark-gfm has no construct
//   toggles, so the wasm side is raised to match it, not vice versa).
//   JS side: unified + remark-parse + remark-mdx + remark-gfm.
//   Exact installed versions are printed with the results.
// - KNOWN ASYMMETRIES (stated, not hidden): (1) remark-mdx parses every
//   MDX expression with acorn and attaches estree data; markdown-rs's
//   default aggressive mode validates braces only — this favors the wasm
//   side on the mdx-heavy doc. (2) The corpus contains no top-level
//   import/export: markdown-rs without `mdx_esm_parse` degrades ESM to
//   paragraphs while remark-mdx parses it, which would compare different
//   work. (3) `parseToAst` is async as shipped (promise + microtask
//   overhead included — that is the real consumer cost); remark's
//   `parse()` is sync.
// - END-TO-END: the measured wasm-side call is the public package
//   function, which includes the wasm call AND `JSON.parse` of the full
//   result document. An approximate JSON.parse-only breakdown line is
//   printed separately (re-stringified result, labeled approximate).
// - Warmup before every measured window; per-iteration samples; mean +
//   p95 reported; one-off init (wasm fetch/compile/instantiate) reported
//   separately, never mixed into steady-state numbers.

import { readFileSync } from "node:fs";

import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkMdx from "remark-mdx";

import { init, parseToAst, version } from "../../dist/index.js";
import { benchCorpus, frontmatteredDoc } from "./fixtures.mjs";
import { byteLength, markdownTable, measure, runGrid } from "./bench-core.mjs";

function installedVersion(pkg) {
  // Resolve through the package dir (pnpm symlink) rather than the exports
  // map — unified-ecosystem packages don't always export ./package.json.
  const url = new URL(`../../node_modules/${pkg}/package.json`, import.meta.url);
  return JSON.parse(readFileSync(url, "utf8")).version;
}

const WASM_OPTIONS = {
  filename: "bench.mdx",
  pipeline: {
    gfm: {
      strikethrough: true,
      table: true,
      autolinkLiteral: true,
      taskListItem: true,
      footnoteDefinition: true,
    },
  },
};

async function main() {
  const remark = unified().use(remarkParse).use(remarkMdx).use(remarkGfm);
  remark.freeze();

  // One-off wasm init cost (fetch + WebAssembly.compile + instantiate),
  // reported separately from steady-state numbers.
  const initStart = performance.now();
  await init();
  const initMs = performance.now() - initStart;

  const corpus = benchCorpus();

  // Pre-flight: both sides must parse the whole corpus cleanly, or the
  // numbers compare error paths. Node counts are printed for transparency
  // (the two mdasts are shaped differently; counts are context, not an
  // equality assertion).
  const countNodes = (node) => {
    let n = 0;
    const walk = (v) => {
      if (Array.isArray(v)) return v.forEach(walk);
      if (v && typeof v === "object") {
        if (typeof v.type === "string") n += 1;
        Object.values(v).forEach(walk);
      }
    };
    walk(node);
    return n;
  };
  const preflight = [];
  for (const { name, doc } of [...corpus, { name: "frontmattered", doc: frontmatteredDoc() }]) {
    const wasmOut = await parseToAst(doc, WASM_OPTIONS);
    if (wasmOut.diagnostics.length > 0 || wasmOut.ast === null) {
      throw new Error(`parseToAst failed on ${name}: ${JSON.stringify(wasmOut.diagnostics)}`);
    }
    const remarkTree = remark.parse(doc); // throws on failure
    preflight.push({
      name,
      wasmNodes: countNodes(wasmOut.ast),
      remarkNodes: countNodes(remarkTree),
    });
  }

  const impls = [
    { name: "remark-parse", fn: (doc) => remark.parse(doc) },
    { name: "parseToAst+JSON.parse", fn: (doc) => parseToAst(doc, WASM_OPTIONS) },
  ];
  const rows = await runGrid(corpus, impls, {
    onProgress: (msg) => process.stderr.write(`[bench] ${msg}\n`),
  });

  // Approximate JSON.parse-only share: re-stringify the full result
  // document once per doc, then time JSON.parse alone on that string.
  const jsonParseRows = [];
  for (const { name, doc, iterations, warmup } of corpus) {
    const resultJson = JSON.stringify(await parseToAst(doc, WASM_OPTIONS));
    const r = await measure(() => JSON.parse(resultJson), { iterations, warmup });
    jsonParseRows.push({ doc: name, jsonBytes: byteLength(resultJson), ...r });
  }

  const versions = {
    node: process.version,
    "@takazudo/zfb-md-wasm": await version(),
    unified: installedVersion("unified"),
    "remark-parse": installedVersion("remark-parse"),
    "remark-gfm": installedVersion("remark-gfm"),
    "remark-mdx": installedVersion("remark-mdx"),
  };

  console.log("## parseToAst vs remark-parse — Node benchmark\n");
  console.log(`Init (one-off, wasm read+compile+instantiate): ${initMs.toFixed(1)} ms\n`);
  console.log("### Steady state (per-parse, warmed)\n");
  console.log(
    markdownTable(
      rows,
      impls.map((i) => i.name),
    ),
  );
  console.log(
    "\n`ratio` = parseToAst+JSON.parse mean ÷ remark-parse mean (>1 means remark is faster).\n",
  );
  console.log("### Approximate JSON.parse-only share (re-stringified result document)\n");
  console.log("| doc | result JSON bytes | JSON.parse mean (ms) | JSON.parse p95 (ms) |");
  console.log("| --- | --- | --- | --- |");
  for (const r of jsonParseRows) {
    console.log(`| ${r.doc} | ${r.jsonBytes} | ${r.meanMs.toFixed(3)} | ${r.p95Ms.toFixed(3)} |`);
  }
  console.log("\n### Pre-flight node counts (context, not an equality claim)\n");
  console.log("| doc | wasm mdast nodes | remark mdast nodes |");
  console.log("| --- | --- | --- |");
  for (const p of preflight) {
    console.log(`| ${p.name} | ${p.wasmNodes} | ${p.remarkNodes} |`);
  }
  console.log("\n### Versions\n");
  for (const [k, v] of Object.entries(versions)) console.log(`- ${k}: ${v}`);
  console.log("\n### Raw JSON\n");
  console.log("```json");
  console.log(JSON.stringify({ initMs, rows, jsonParseRows, preflight, versions }, null, 2));
  console.log("```");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
