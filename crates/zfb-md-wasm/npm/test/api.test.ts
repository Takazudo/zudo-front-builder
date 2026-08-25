import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, it, expect } from "vitest";

// Tests run against the BUILT package (`dist/`), not `src/`: this is what
// `pnpm build && pnpm test` (and `prepublishOnly`) exercise, and it is the
// artifact consumers actually load. `dist/index.js` is plain tsc output, so
// the trap/re-init machinery's query-string dynamic re-import behaves exactly
// as it does for a real consumer under Node -- no bundler in the middle.
import {
  compile,
  highlightCode,
  renderHtml,
  version,
  init,
  ZfbMdWasmTrapError,
  ZfbMdWasmTrapRecoveryLimitError,
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
} from "../dist/index.js";

interface DiagnosticFixture {
  slug: string;
  source: string;
  options: { filename: string };
  line: number;
  column: number;
}

const diagnosticFixtures = (
  JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "..",
        "..",
        "tests",
        "fixtures",
        "parse-to-ast",
        "diagnostics.json",
      ),
      "utf8",
    ),
  ) as { fixtures: DiagnosticFixture[] }
).fixtures;

function nestedYamlFrontmatter(depth: number): string {
  let yaml = "root:\n";
  for (let i = 0; i < depth; i += 1) {
    yaml += `${"  ".repeat(i + 1)}k${i}:\n`;
  }
  yaml += `${"  ".repeat(depth + 1)}leaf: ok\n`;
  return `---\n${yaml}---\n# Body\n`;
}

describe("renderHtml (md -> HTML, no SWC)", () => {
  it("renders markdown and returns parsed frontmatter VALUES", async () => {
    const out = await renderHtml(
      "---\ntitle: Hi\ncount: 3\n---\n\n# Heading\n\nSome **bold** text.\n",
      {
        filename: "post.md",
      },
    );
    expect(out.html).toBe("<h1>Heading</h1><p>Some <strong>bold</strong> text.</p>");
    // frontmatter asserted by value, not just shape
    expect(out.frontmatter).toEqual({ title: "Hi", count: 3 });
    expect(out.diagnostics).toHaveLength(0);
  });

  it("returns null frontmatter when the source has none", async () => {
    const out = await renderHtml("# Just a heading\n");
    expect(out.html).toBe("<h1>Just a heading</h1>");
    expect(out.frontmatter).toBeNull();
    expect(out.diagnostics).toHaveLength(0);
  });

  it("infers CommonMark for .md and keeps MDX parsing for .mdx", async () => {
    const markdown = await renderHtml("Budget <8 ms\n", { filename: "preview.md" });
    expect(markdown).toMatchObject({
      html: "<p>Budget &lt;8 ms</p>",
      diagnostics: [],
    });

    const mdx = await renderHtml("Budget <8 ms\n", { filename: "preview.mdx" });
    expect(mdx.html).toBeNull();
    expect(mdx.diagnostics).toEqual([
      expect.objectContaining({ source: "markdown", severity: "error" }),
    ]);
  });

  it("allows an explicit dialect override without losing GFM switches", async () => {
    const markdown = await renderHtml("Budget <8 ms\n\n~~old~~\n", {
      filename: "preview.mdx",
      dialect: "markdown",
      pipeline: { gfm: { strikethrough: true } },
    });
    expect(markdown).toMatchObject({
      html: "<p>Budget &lt;8 ms</p><p><del>old</del></p>",
      diagnostics: [],
    });

    const mdx = await renderHtml("Budget <8 ms\n", {
      filename: "preview.md",
      dialect: "mdx",
    });
    expect(mdx.html).toBeNull();
    expect(mdx.diagnostics[0]).toMatchObject({ source: "markdown" });
  });

  it("applies a feature toggle from the resolved config JSON (github alerts)", async () => {
    const out = await renderHtml("> [!NOTE]\n> hey\n", {
      pipeline: { features: { githubAlerts: true } },
    });
    expect(out.html).toContain("<Note>");
    expect(out.diagnostics).toHaveLength(0);
  });

  it("preserves rendered HTML inside directive and alert component bodies", async () => {
    const out = await renderHtml(
      "# Title\n\n" +
        ":::note[Heads up]\n" +
        "First paragraph with **bold**, `code` and a [link](./other.md).\n\n" +
        "Second paragraph.\n" +
        ":::\n\n" +
        "> [!IMPORTANT]\n" +
        "> Alert body with *emphasis*.\n",
      {
        filename: "preview.mdx",
        pipeline: {
          features: {
            directives: { note: "Note" },
            githubAlerts: true,
          },
        },
      },
    );

    expect(out.html).toBe(
      '<h1>Title</h1><Note title="Heads up">' +
        "<p>First paragraph with <strong>bold</strong>, <code>code</code> and " +
        'a <a href="./other.md">link</a>.</p>' +
        "<p>Second paragraph.</p></Note>" +
        "<Important><p>Alert body with <em>emphasis</em>.</p></Important>",
    );
    expect(out.diagnostics).toHaveLength(0);
  });
});

describe("compile (mdx -> ES-module JS via SWC)", () => {
  it("compiles mdx with frontmatter, a PascalCase component and an expression", async () => {
    const out = await compile("---\ntitle: T\ncount: 3\n---\n\n# H\n\n<Foo/>\n\n{1 + 2}\n", {
      filename: "post.mdx",
    });
    const code = out.code ?? "";
    expect(code).toContain("export default function MDXContent");
    // frontmatter asserted by value, not just shape
    expect(out.frontmatter).toEqual({ title: "T", count: 3 });
    expect(out.diagnostics).toHaveLength(0);
  });

  it("defaults to the preact jsx runtime, but Fragment still comes from react/jsx-runtime", async () => {
    // This asymmetry is zfb's production emitter shape (parity-correct, not a
    // bug): jsx factory is preact's, Fragment is react's. A browser consumer
    // on preact MUST alias react/jsx-runtime -> preact/jsx-runtime. Locking it
    // in here so a future emitter change is a conscious decision. See README
    // "Evaluating compiled modules in a browser".
    const out = await compile("# hi\n", { filename: "p.mdx" });
    const code = out.code ?? "";
    expect(code).toContain('from "preact/jsx-runtime"');
    expect(code).toContain('import { Fragment as _Fragment } from "react/jsx-runtime"');
  });

  it("honors jsxRuntime: react", async () => {
    const out = await compile("# hi\n", { filename: "p.mdx", jsxRuntime: "react" });
    expect(out.code ?? "").toContain('from "react/jsx-runtime"');
    expect(out.diagnostics).toHaveLength(0);
  });
});

// pipeline.codeHighlight (Highlight Tokens epic zfb#1528, wasm routing sub
// zfb#1852, npm proof sub zfb#1853). The class-emission engine itself
// (prefix, roles, multiline state, fingerprint) is proven by the Rust
// crate's own suites (zfb-content's `class_mode_pipeline.rs` /
// `tests/facade.rs`) -- this describe block's job is narrower: prove the
// `codeHighlight` JSON knob reaches the class arm through the ACTUAL BUILT
// wasm boundary (`dist/index.js`) for both `renderHtml` and `compile`, and
// that the pre-existing inline behaviour is unaffected. Level 3 (build
// output) per project testing discipline -- same rationale as the rest of
// this file.
const CLASS_MODE_FENCE = "```rust\nfn main() {}\n```\n";

describe("pipeline.codeHighlight (class-mode routing through the built wasm)", () => {
  it("renderHtml: mode 'class' emits hi-* role spans with zero inline styles", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { codeHighlight: { mode: "class" } },
    });
    expect(out.html).toContain('class="hi-root"');
    expect(out.html).toContain('class="hi-kw"');
    expect(out.html).toContain('class="hi-fn"');
    expect(out.html).not.toContain("style=");
    expect(out.html).not.toContain("syntect-");
    expect(out.diagnostics).toHaveLength(0);
  });

  it("renderHtml: mode 'class' honors a custom classPrefix and roleClasses override", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: {
        codeHighlight: {
          mode: "class",
          classPrefix: "token-",
          roleClasses: { keyword: "text-violet-600 dark:text-violet-400" },
        },
      },
    });
    expect(out.html).toContain('class="token-root"');
    expect(out.html).toContain('class="text-violet-600 dark:text-violet-400"');
    expect(out.html).not.toContain('class="token-kw"');
    expect(out.html).not.toContain("style=");
    expect(out.diagnostics).toHaveLength(0);
  });

  it("compile: mode 'class' emits hi-* classes in the MDXContent module, zero inline styles", async () => {
    const out = await compile(CLASS_MODE_FENCE, {
      filename: "p.mdx",
      pipeline: { codeHighlight: { mode: "class" } },
    });
    const code = out.code ?? "";
    // The pre element itself carries the hi-root class as a real JSX prop
    // (not baked into the dangerouslySetInnerHTML string).
    expect(code).toContain('class: "hi-root"');
    // Token spans stay inside the raw-HTML span (same shape as inline
    // mode's syntect markup) but carry hi-* classes, not inline styles.
    expect(code).toContain('class=\\"hi-kw\\"');
    expect(code).toContain('class=\\"hi-fn\\"');
    expect(code).not.toContain("style=");
    expect(code).not.toContain("syntect-");
    expect(out.diagnostics).toHaveLength(0);
  });

  it("mode 'class' combined with a non-null top-level theme is a structured options diagnostic, not a throw", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { theme: "InspiredGitHub", codeHighlight: { mode: "class" } },
    });
    expect(out.html).toBeNull();
    expect(out.diagnostics).toEqual([
      expect.objectContaining({
        severity: "error",
        source: "options",
        message: expect.stringContaining(
          'codeHighlight.mode "class" is mutually exclusive with theme',
        ),
      }),
    ]);
  });

  it("mode 'class' combined with an explicit theme: null is NOT a conflict (null == absent)", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { theme: null, codeHighlight: { mode: "class" } },
    });
    expect(out.html).toContain('class="hi-root"');
    expect(out.html).toContain('class="hi-kw"');
    expect(out.diagnostics).toHaveLength(0);
  });

  // Legacy-matrix: every non-class shape must reproduce the pre-#1852
  // inline byte-for-byte, both to prove nothing regressed and to pin the
  // default so a future change to the knob's default is a conscious edit.
  // `toBe`d against a pinned exact string (not `stringContaining`) so a
  // regression in token grouping, colors, or wrapper markup that still
  // happens to keep the syntect class name and *a* color style would fail
  // here rather than passing (codex review finding, zfb#1853).
  const EXACT_INLINE_HTML =
    '<pre class="syntect-base16-ocean-dark"><code><span class="line">' +
    '<span style="color:#b48ead;">fn </span>' +
    '<span style="color:#8fa1b3;">main</span>' +
    '<span style="color:#c0c5ce;">() {}</span>' +
    "</span></code></pre>";

  it("legacy matrix: absent codeHighlight defaults to inline", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {});
    expect(out.html).toBe(EXACT_INLINE_HTML);
  });

  it("legacy matrix: explicit null codeHighlight defaults to inline", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { codeHighlight: null },
    });
    expect(out.html).toBe(EXACT_INLINE_HTML);
  });

  it("legacy matrix: explicit mode 'inline' reproduces the same shape", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { codeHighlight: { mode: "inline" } },
    });
    expect(out.html).toBe(EXACT_INLINE_HTML);
  });

  it("legacy matrix: inline mode still honors a top-level theme (no conflict)", async () => {
    const out = await renderHtml(CLASS_MODE_FENCE, {
      pipeline: { theme: "InspiredGitHub" },
    });
    expect(out.html).toContain('class="syntect-inspiredgithub"');
    expect(out.html).toContain("style=");
    expect(out.diagnostics).toHaveLength(0);
  });
});

describe("highlightCode (arbitrary source -> semantic class markup)", () => {
  it("renders HTML, CSS, and JavaScript without Markdown fencing", async () => {
    for (const [language, code] of [
      ["html", '<main data-x="a & b">hello</main>'],
      ["css", ".button { color: red; }"],
      ["javascript", "const value = '<tag>';"],
    ] as const) {
      const out = await highlightCode(code, { language });
      expect(out.html).toMatch(/^<pre class="hi-root"><code>/);
      expect(out.html).toContain('<span class="line">');
      expect(out.diagnostics).toEqual([]);
    }
  });

  it("maps full-name role overrides and custom prefixes", async () => {
    const out = await highlightCode("const answer = 42;", {
      language: "javascript",
      classPrefix: "token-",
      roleClasses: { keyword: "text-violet-600 dark:text-violet-400" },
    });
    expect(out.html).toMatch(/^<pre class="token-root"><code>/);
    expect(out.html).toContain('class="text-violet-600 dark:text-violet-400">const</span>');
    expect(out.html).not.toContain("token-kw");
    expect(out.diagnostics).toEqual([]);
  });

  it("returns escaped fallback markup plus a warning for an unknown language", async () => {
    const out = await highlightCode("<tag>&", { language: "not-a-bundled-syntax" });
    expect(out.html).toBe(
      '<pre class="hi-root"><code><span class="line">&lt;tag&gt;&amp;</span></code></pre>',
    );
    expect(out.diagnostics).toEqual([
      expect.objectContaining({
        severity: "warning",
        source: "highlight",
        line: null,
        column: null,
      }),
    ]);
  });

  it("returns structured option errors rather than throwing", async () => {
    // @ts-expect-error deliberately omit the required direct language option
    const missing = await highlightCode("const x = 1;", {});
    expect(missing.html).toBeNull();
    expect(missing.diagnostics[0]).toMatchObject({ severity: "error", source: "options" });

    // @ts-expect-error deliberately exercise the Rust deny_unknown_fields boundary
    const unknown = await highlightCode("const x = 1;", { language: "javascript", bogus: true });
    expect(unknown.html).toBeNull();
    expect(unknown.diagnostics[0]).toMatchObject({ severity: "error", source: "options" });
  });
});

describe("expected failures surface as structured diagnostics (never a throw)", () => {
  it("malformed MDX -> markdown diagnostic, code null, frontmatter still extracted", async () => {
    const out = await compile("---\ntitle: x\n---\n<Card>\n\nsome text\n", { filename: "bad.mdx" });
    expect(out.code).toBeNull();
    // frontmatter parsed before the body failed
    expect(out.frontmatter).toEqual({ title: "x" });
    expect(out.diagnostics).toHaveLength(1);
    const [diag] = out.diagnostics;
    expect(diag.severity).toBe("error");
    expect(diag.source).toBe("markdown");
    expect(diag.line).toBeTypeOf("number");
  });

  it("unknown options field -> options diagnostic (deny_unknown_fields)", async () => {
    // @ts-expect-error deliberately passing an unknown field to exercise the Rust validator
    const out = await compile("# hi\n", { filename: "p.mdx", bogus: 1 });
    expect(out.code).toBeNull();
    expect(out.diagnostics[0]?.source).toBe("options");
    expect(out.diagnostics[0]?.message).toContain("unknown field");
  });

  it("a filename not ending in .md/.mdx -> options diagnostic", async () => {
    const out = await compile("# hi\n", { filename: "p.txt" });
    expect(out.code).toBeNull();
    expect(out.diagnostics[0]?.source).toBe("options");
  });

  it("deep YAML frontmatter returns a diagnostic rather than trapping", async () => {
    const before = __getTrapRecoveryStateForTests();

    const out = await renderHtml(nestedYamlFrontmatter(512), { filename: "deep.md" });

    const after = __getTrapRecoveryStateForTests();
    expect(out.html).toBeNull();
    expect(out.frontmatter).toBeNull();
    expect(out.diagnostics).toHaveLength(1);
    expect(out.diagnostics[0]?.source).toBe("frontmatter");
    expect(out.diagnostics[0]?.message).toContain("recursion limit exceeded");
    expect(after.trapRecoveriesStarted).toBe(before.trapRecoveriesStarted);
  });

  it("reasonable YAML frontmatter depth still parses", async () => {
    const out = await renderHtml(nestedYamlFrontmatter(20), { filename: "reasonable.md" });

    expect(out.html).toBe("<h1>Body</h1>");
    expect(out.diagnostics).toHaveLength(0);
    let cursor = out.frontmatter as Record<string, unknown>;
    cursor = cursor.root as Record<string, unknown>;
    for (let i = 0; i < 20; i += 1) {
      cursor = cursor[`k${i}`] as Record<string, unknown>;
    }
    expect(cursor.leaf).toBe("ok");
  });
});

describe("markdown diagnostics use original-source UTF-16 coordinates", () => {
  for (const fixture of diagnosticFixtures) {
    it(`${fixture.slug}: compile and renderHtml agree`, async () => {
      const [compiled, rendered] = await Promise.all([
        compile(fixture.source, fixture.options),
        renderHtml(fixture.source, fixture.options),
      ]);

      for (const [entrypoint, out] of [
        ["compile", compiled],
        ["renderHtml", rendered],
      ] as const) {
        expect(out.diagnostics, `${entrypoint} must return one parse failure`).toHaveLength(1);
        expect(out.diagnostics[0]).toMatchObject({
          source: "markdown",
          line: fixture.line,
          column: fixture.column,
        });
      }
    });
  }
});

describe("version()", () => {
  it("returns a package semver string", async () => {
    await init();
    const semverPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
    expect(await version()).toMatch(semverPattern);
  });
});

describe("trap / auto-re-init contract", () => {
  it("a wasm trap throws ZfbMdWasmTrapError and the next call transparently recovers", async () => {
    await init();
    // Force a genuine wasm RuntimeError through the internal Rust panic hook
    // -- the same exception class a real Rust panic->trap produces, which is
    // what the wrapper's catch clause keys on.
    await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);

    // The poisoned instance was dropped and re-instantiated in the background;
    // a subsequent normal call must succeed against the fresh instance.
    const out = await renderHtml("# recovered\n");
    expect(out.html).toBe("<h1>recovered</h1>");
    expect(out.diagnostics).toHaveLength(0);

    // `highlightCode` uses the same generic call path and therefore the same
    // recovered instance rather than a separate initialization/cache layer.
    const highlighted = await highlightCode("const recovered = true;", { language: "javascript" });
    expect(highlighted.html).toContain("hi-kw");
    expect(highlighted.diagnostics).toEqual([]);
  });

  it("single-flights concurrent trap recovery to one fresh instantiation", async () => {
    await init();
    const before = __getTrapRecoveryStateForTests();

    const trapA = __forceTrapForTests();
    const trapB = __forceTrapForTests();

    await Promise.all([
      expect(trapA).rejects.toBeInstanceOf(ZfbMdWasmTrapError),
      expect(trapB).rejects.toBeInstanceOf(ZfbMdWasmTrapError),
    ]);

    const after = __getTrapRecoveryStateForTests();
    expect(after.trapRecoveriesStarted - before.trapRecoveriesStarted).toBe(1);
    expect(after.freshInstanceStarts - before.freshInstanceStarts).toBe(1);

    const out = await renderHtml("# recovered after concurrent traps\n");
    expect(out.html).toBe("<h1>recovered after concurrent traps</h1>");
    expect(out.diagnostics).toHaveLength(0);
  });

  it("stops recovering after the cap and mints no further module records", async () => {
    await init();
    const before = __getTrapRecoveryStateForTests();
    const remainingRecoveries = before.maxTrapRecoveries - before.trapRecoveriesStarted;
    expect(remainingRecoveries).toBeGreaterThan(0);

    for (let i = 0; i < remainingRecoveries; i += 1) {
      await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);
    }

    const atCap = __getTrapRecoveryStateForTests();
    expect(atCap.trapRecoveriesStarted).toBe(atCap.maxTrapRecoveries);
    const freshInstanceStartsAtCap = atCap.freshInstanceStarts;

    await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapRecoveryLimitError);
    const terminal = __getTrapRecoveryStateForTests();
    expect(terminal.terminal).toBe(true);
    expect(terminal.freshInstanceStarts).toBe(freshInstanceStartsAtCap);

    await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapRecoveryLimitError);
    expect(__getTrapRecoveryStateForTests().freshInstanceStarts).toBe(freshInstanceStartsAtCap);
  });
});
