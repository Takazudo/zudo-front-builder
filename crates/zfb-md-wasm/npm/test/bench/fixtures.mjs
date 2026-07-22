// PROTOTYPE benchmark fixtures (zfb#1855, epic zfb#1854 — parseToAst
// go/no-go spike). Deterministic in-memory document builders shared by the
// Node runner (`bench-parse-ast.mjs`) and the browser harness
// (`browser/`). Built in-memory (no committed 200 KB blobs) from fixed
// string pools — every call returns byte-identical documents.
//
// Capability-parity constraints baked into the corpus (see the runner's
// methodology header): both sides parse as MDX + full GFM, so
// - prose never uses raw `<` or `{` outside deliberate JSX/expressions;
// - MDX expressions are VALID JS (remark-mdx runs acorn on them;
//   markdown-rs's default aggressive mode does not — asymmetry noted in
//   the methodology);
// - no top-level `import`/`export` (markdown-rs without `mdx_esm_parse`
//   treats them as paragraphs while remark-mdx parses real ESM — that
//   divergence would make the comparison dishonest).

const PROSE = [
  "The build pipeline walks every entry in dependency order and records the observed hash so later passes can skip untouched files without re-reading them from disk.",
  "Steady-state performance only matters once the cold path is out of the way, which is why the loader keeps its warmup pass separate from the measured window.",
  "A parser that reports positions against the stripped body will confuse every editor integration downstream, so the offsets are shifted back before anything leaves the boundary.",
  "Content collections resolve their frontmatter first, then hand the remaining body to the same pipeline the bundler uses, keeping dev and build output byte-identical.",
  "Most documents are short, but the occasional imported changelog lands in the hundreds of kilobytes, and the parser has to stay predictable across that whole range.",
  "Serialization cost is the classic objection to crossing a wasm boundary with a tree this large, which is exactly the question this corpus exists to answer.",
  "The watcher batches events for a few milliseconds before invalidating, because editors love to write the same file three times in a row during a single save.",
  "Every diagnostic carries a one-based line and column pointing into the original source, frontmatter included, so error overlays can highlight the right span.",
];

const INLINE_DECORATIONS = [
  (s) => `*${s}*`,
  (s) => `**${s}**`,
  (s) => `\`${s}\``,
  (s) => `~~${s}~~`,
  (s) => `[${s}](https://example.com/docs/${s.replace(/[^a-z]+/gi, "-").toLowerCase()})`,
];

function sentence(i) {
  return PROSE[i % PROSE.length];
}

function decoratedParagraph(i) {
  const base = sentence(i);
  const words = sentence(i + 3).split(" ");
  const picked = words[i % words.length].replace(/[^A-Za-z-]/g, "") || "token";
  const decorated = INLINE_DECORATIONS[i % INLINE_DECORATIONS.length](picked);
  return `${base} Along the way it touches ${decorated} and keeps going: ${sentence(i + 5)}`;
}

function section(
  index,
  {
    withTable = false,
    withList = false,
    withQuote = false,
    withFence = false,
    withFootnote = false,
  } = {},
) {
  const parts = [
    `## Section ${index + 1}: ${sentence(index).split(" ").slice(0, 4).join(" ")}`,
    "",
  ];
  parts.push(decoratedParagraph(index), "");
  if (withList) {
    parts.push(
      `- first point about ${sentence(index + 1).split(" ")[1]}`,
      `- second point with **emphasis** and \`code\``,
      `- [ ] an open task`,
      `- [x] a closed task`,
      "",
    );
  }
  if (withTable) {
    parts.push(
      "| stage | cold (ms) | warm (ms) |",
      "| ----- | --------- | --------- |",
      `| parse | ${100 + index} | ${10 + (index % 7)} |`,
      `| serialize | ${40 + index} | ${5 + (index % 5)} |`,
      "",
    );
  }
  if (withQuote) {
    parts.push(`> ${sentence(index + 2)}`, "");
  }
  if (withFence) {
    parts.push(
      "```js",
      `export function step${index}(input) {`,
      `  return input.map((x) => x * ${index + 2});`,
      "}",
      "```",
      "",
    );
  }
  if (withFootnote) {
    parts.push(
      `The measured window excludes warmup[^note${index}].`,
      "",
      `[^note${index}]: Warmup iterations are discarded before statistics are computed.`,
      "",
    );
  }
  parts.push(
    `${decoratedParagraph(index + 4)} See https://example.com/ref/${index} for the raw data.`,
    "",
  );
  return parts.join("\n");
}

function repeatSections(count, offset = 0) {
  const out = [];
  for (let i = 0; i < count; i += 1) {
    out.push(
      section(i + offset, {
        withList: i % 3 === 0,
        withTable: i % 4 === 1,
        withQuote: i % 5 === 2,
        withFence: i % 6 === 3,
        withFootnote: i % 7 === 4,
      }),
    );
  }
  return out.join("\n");
}

/** ~1 KB: one short article opening. */
export function smallDoc() {
  return [
    "# Small document",
    "",
    decoratedParagraph(0),
    "",
    "- a list item",
    "- another with `code`",
    "",
    decoratedParagraph(1),
    "",
    `> ${sentence(2)}`,
    "",
    decoratedParagraph(3),
    "",
  ].join("\n");
}

/** ~20 KB: varied real-prose sections (lists, tables, quotes, sparse fences, footnotes). */
export function mediumDoc() {
  return `# Medium document\n\n${repeatSections(22)}`;
}

/** ~200 KB: the medium generator scaled up with distinct section offsets. */
export function largeDoc() {
  const chapters = [];
  for (let c = 0; c < 10; c += 1) {
    chapters.push(`# Chapter ${c + 1}\n\n${repeatSections(22, c * 22)}`);
  }
  return chapters.join("\n");
}

/** ~25 KB: dominated by fenced code blocks in several languages. */
export function codeHeavyDoc() {
  const parts = ["# Code-heavy document", ""];
  const fences = [
    (i) => [
      "```rust",
      `fn pass_${i}(input: &mut Vec<u64>) -> u64 {`,
      `    input.iter().map(|x| x.wrapping_mul(${i + 1})).sum()`,
      "}",
      "```",
    ],
    (i) => [
      "```js",
      `export const table${i} = new Map(`,
      `  Array.from({ length: ${i + 8} }, (v, k) => [k, k * ${i + 2}]),`,
      ");",
      "```",
    ],
    (i) => [
      "```css",
      `.stage-${i} {`,
      `  grid-template-columns: repeat(${(i % 4) + 1}, minmax(0, 1fr));`,
      "  gap: 0.5rem;",
      "}",
      "```",
    ],
    (i) => [
      "```json",
      "{",
      `  "stage": ${i},`,
      `  "cold_ms": ${100 + i},`,
      `  "warm_ms": ${10 + (i % 9)}`,
      "}",
      "```",
    ],
    (i) => [
      "```html",
      `<section class="pass-${i}">`,
      `  <h2>Pass ${i}</h2>`,
      `  <p>Rendered output for stage ${i}.</p>`,
      "</section>",
      "```",
    ],
  ];
  for (let i = 0; i < 90; i += 1) {
    parts.push(`## Snippet ${i + 1}`, "", sentence(i), "", ...fences[i % fences.length](i), "");
  }
  return parts.join("\n");
}

/**
 * ~15 KB: JSX-element and expression heavy. Asymmetry caveat (stated in the
 * methodology): remark-mdx parses each expression with acorn and attaches
 * estree data; markdown-rs's default aggressive mode validates braces only.
 */
export function mdxHeavyDoc() {
  const parts = ["# MDX-heavy document", ""];
  for (let i = 0; i < 30; i += 1) {
    parts.push(
      `## Component pass ${i + 1}`,
      "",
      sentence(i),
      "",
      `<Card label="pass-${i}" count={${i} * 3} pinned={${i % 2 === 0}}>`,
      `  Rendered total so far: {${i} + ${i + 1}}.`,
      `  <Badge kind="info" weight={${(i % 5) + 1}} />`,
      "</Card>",
      "",
      `Inline math-ish expression {${i} * 7} inside prose, plus ${INLINE_DECORATIONS[i % 5]("markers")}.`,
      "",
      `{ /* standalone flow expression ${i} */ }`,
      "",
    );
  }
  return parts.join("\n");
}

/** Frontmatter'd variant used by position-related smoke checks. */
export function frontmatteredDoc() {
  return `---\ntitle: Bench fixture\nrevision: 7\n---\n\n${smallDoc()}`;
}

/** The benchmark corpus, in presentation order. */
export function benchCorpus() {
  return [
    { name: "small", doc: smallDoc(), iterations: 200, warmup: 25 },
    { name: "medium", doc: mediumDoc(), iterations: 100, warmup: 25 },
    { name: "large", doc: largeDoc(), iterations: 30, warmup: 5 },
    { name: "code-heavy", doc: codeHeavyDoc(), iterations: 100, warmup: 25 },
    { name: "mdx-heavy", doc: mdxHeavyDoc(), iterations: 100, warmup: 25 },
  ];
}
