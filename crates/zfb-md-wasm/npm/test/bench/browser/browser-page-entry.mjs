// Browser bench page entry (parseToAst, zfb#1857, epic zfb#1854). Bundled by
// `run-browser-bench.mjs` with the same esbuild loader config the packaged
// browser entry is proven against in `test/package-browser.test.ts`
// (`--loader:.zfb-resource.mjs=file --loader:.wasm=file`). Runs the exact
// same capability-matched grid as the Node runner — same fixtures, same
// bench core, same options — inside real Chromium.

import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkMdx from "remark-mdx";

import { init, parseToAst } from "../../../dist/browser.js";
import { benchCorpus } from "../fixtures.mjs";
import { runGrid } from "../bench-core.mjs";

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

window.__runBench = async () => {
  const remark = unified().use(remarkParse).use(remarkMdx).use(remarkGfm);
  remark.freeze();

  const initStart = performance.now();
  await init();
  const initMs = performance.now() - initStart;

  const corpus = benchCorpus();
  for (const { name, doc } of corpus) {
    const out = await parseToAst(doc, WASM_OPTIONS);
    if (out.diagnostics.length > 0 || out.ast === null) {
      throw new Error(`parseToAst failed on ${name}: ${JSON.stringify(out.diagnostics)}`);
    }
    remark.parse(doc); // throws on failure
  }

  const impls = [
    { name: "remark-parse", fn: (doc) => remark.parse(doc) },
    { name: "parseToAst+JSON.parse", fn: (doc) => parseToAst(doc, WASM_OPTIONS) },
  ];
  const rows = await runGrid(corpus, impls, {
    onProgress: (msg) => console.log(`[bench] ${msg}`),
  });
  return { initMs, rows, implNames: impls.map((i) => i.name) };
};

document.body.append("bench page ready");
