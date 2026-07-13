import { describe, it, expect } from "vitest";

// Tests run against the BUILT package (`dist/`), not `src/`: this is what
// `pnpm build && pnpm test` (and `prepublishOnly`) exercise, and it is the
// artifact consumers actually load. `dist/index.js` is plain tsc output, so
// the trap/re-init machinery's query-string dynamic re-import behaves exactly
// as it does for a real consumer under Node -- no bundler in the middle.
import {
  compile,
  renderHtml,
  version,
  init,
  ZfbMdWasmTrapError,
  ZfbMdWasmTrapRecoveryLimitError,
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
} from "../dist/index.js";

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

  it("applies a feature toggle from the resolved config JSON (github alerts)", async () => {
    const out = await renderHtml("> [!NOTE]\n> hey\n", {
      pipeline: { features: { githubAlerts: true } },
    });
    expect(out.html).toContain("<Note>");
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
    // Force a genuine wasm RuntimeError (memory access out of bounds) via the
    // internal test hook -- the same exception class a real Rust panic->trap
    // produces, which is what the wrapper's catch clause keys on.
    await expect(__forceTrapForTests()).rejects.toBeInstanceOf(ZfbMdWasmTrapError);

    // The poisoned instance was dropped and re-instantiated in the background;
    // a subsequent normal call must succeed against the fresh instance.
    const out = await renderHtml("# recovered\n");
    expect(out.html).toBe("<h1>recovered</h1>");
    expect(out.diagnostics).toHaveLength(0);
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
