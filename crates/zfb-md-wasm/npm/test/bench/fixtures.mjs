// parseToAst benchmark fixtures (zfb#1857, epic zfb#1854). Deterministic
// in-memory document builders shared by the Node runner
// (`bench-parse-ast.mjs`) and the browser harness (`browser/`). Built
// in-memory (no committed 200 KB blobs) from fixed string pools — every
// call returns byte-identical documents.
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

// CJK (Japanese) prose pool + emoji, for `cjkHeavyDoc()` (zfb#1857): every
// fixture above is pure ASCII, so it takes the UTF-16 conversion's ASCII
// fast path and never measures the conversion itself (see `lib.rs`'s
// `Utf16Positions`). This fixture puts the conversion cost inside the
// measured window -- emoji specifically, not just CJK, since a scalar value
// outside the Basic Multilingual Plane needs a UTF-16 surrogate pair (the
// same reason the correctness fixtures use emoji, not CJK alone).
const CJK_PROSE = [
  "ビルドパイプラインは依存関係の順序ですべてのエントリを処理し、観測したハッシュを記録することで、変更されていないファイルを再読み込みせずにスキップできるようにする。",
  "コールドパスが終わるまでは定常状態の性能は問題にならないため、ローダーはウォームアップ処理を計測対象の区間から切り離している。",
  "本文から取り除いた位置情報を報告するパーサーは、下流のすべてのエディタ連携を混乱させてしまうため、境界を出る前にオフセットを元に戻している。",
  "コンテンツコレクションはまずフロントマターを解決し、残った本文を同じパイプラインへ渡すことで、開発時とビルド時の出力をバイト単位で一致させる。",
  "ほとんどのドキュメントは短いが、たまにインポートされる変更履歴が数百キロバイトに達することもあり、パーサーはその全域で予測可能な挙動を保つ必要がある。",
  "巨大な木構造を wasm 境界越しにやり取りするときのシリアライズコストこそが古典的な懸念であり、このコーパスはまさにその問いに答えるために存在する。",
  "エディタは保存のたびに同じファイルを何度も書き込みがちなので、ウォッチャーは無効化する前に数ミリ秒だけイベントをまとめてから処理する。",
  "各診断情報はフロントマターを含む元のソースを指す 1 始まりの行と列を持つため、エラー表示は正しい範囲を強調できる。",
];

const CJK_INLINE_DECORATIONS = [
  (s) => `*${s}*`,
  (s) => `**${s}**`,
  (s) => `\`${s}\``,
  (s) => `~~${s}~~`,
];

const CJK_EMOJI = ["🚀", "✨", "📄", "🎉", "🧭", "🔥"];

function cjkSentence(i) {
  return CJK_PROSE[i % CJK_PROSE.length];
}

function cjkSection(index) {
  const emoji = CJK_EMOJI[index % CJK_EMOJI.length];
  const decorated = CJK_INLINE_DECORATIONS[index % CJK_INLINE_DECORATIONS.length](
    cjkSentence(index + 1).slice(0, 8),
  );
  const parts = [
    `## セクション ${index + 1}`,
    "",
    `${cjkSentence(index)} ${emoji} ${decorated} を含む一文。`,
    "",
  ];
  if (index % 3 === 0) {
    parts.push(`- 箇条書きその一 ${emoji}`, "- **強調** と `コード` を含む箇条書きその二", "");
  }
  if (index % 4 === 1) {
    parts.push(`> ${cjkSentence(index + 2)}`, "");
  }
  return parts.join("\n");
}

/**
 * ~20 KB: Japanese prose sections with sprinkled emoji, no MDX/GFM
 * constructs (parity with the plain-markdown non-ASCII fixture the
 * correctness suite shares with the Rust side). Measures the UTF-16
 * position-conversion cost the ASCII fixtures above cannot.
 */
export function cjkHeavyDoc() {
  const parts = ["# CJK ドキュメント", ""];
  for (let i = 0; i < 24; i += 1) {
    parts.push(cjkSection(i));
  }
  return parts.join("\n");
}

/** The benchmark corpus, in presentation order. */
export function benchCorpus() {
  return [
    { name: "small", doc: smallDoc(), iterations: 200, warmup: 25 },
    { name: "medium", doc: mediumDoc(), iterations: 100, warmup: 25 },
    { name: "large", doc: largeDoc(), iterations: 30, warmup: 5 },
    { name: "code-heavy", doc: codeHeavyDoc(), iterations: 100, warmup: 25 },
    { name: "mdx-heavy", doc: mdxHeavyDoc(), iterations: 100, warmup: 25 },
    { name: "cjk-heavy", doc: cjkHeavyDoc(), iterations: 100, warmup: 25 },
  ];
}
