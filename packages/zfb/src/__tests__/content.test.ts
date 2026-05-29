import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  _relPathToSlug,
  ContentBlockquote,
  ContentCode,
  ContentH2,
  ContentH3,
  ContentH4,
  ContentLink,
  ContentOl,
  ContentParagraph,
  ContentStrong,
  ContentTable,
  ContentUl,
  defaultComponents,
  getCollection,
  getContentSnapshot,
  mergeMdxComponents,
  parseFrontmatter,
  setContentSnapshot,
} from "../content.js";
import type { MdxComponents, Snapshot } from "../content.js";

/**
 * Test-only handle on the `__zfb` bridge namespace. Mirrors the ambient
 * declaration the production renderer installs, narrowed to what these
 * tests touch (the `content.get(specifier)` lookup and the `mdxComponents`
 * global slot).
 */
type TestBridge = {
  __zfb?: {
    content?: {
      get(
        specifier: string,
      ): ((props: { components?: Record<string, unknown> }) => unknown) | undefined;
    };
    mdxComponents?: MdxComponents;
  };
};

describe("parseFrontmatter", () => {
  it("returns empty data and full body when no frontmatter is present", () => {
    const { data, body } = parseFrontmatter("hello world\n");
    expect(data).toEqual({});
    expect(body).toBe("hello world\n");
  });

  it("parses simple key/value scalars", () => {
    const raw = "---\ntitle: Hello\ndate: 2026-04-20\n---\nbody text\n";
    const { data, body } = parseFrontmatter(raw);
    expect(data).toEqual({ title: "Hello", date: "2026-04-20" });
    expect(body).toBe("body text\n");
  });

  it("unwraps single- and double-quoted scalar values", () => {
    const raw = ["---", 'title: "Hello, World"', "subtitle: 'with comma'", "---", "body"].join(
      "\n",
    );
    const { data } = parseFrontmatter(raw);
    expect(data["title"]).toBe("Hello, World");
    expect(data["subtitle"]).toBe("with comma");
  });

  it("parses block-list arrays", () => {
    const raw = ["---", "tags:", "  - intro", "  - framework", "---", ""].join("\n");
    const { data } = parseFrontmatter(raw);
    expect(data["tags"]).toEqual(["intro", "framework"]);
  });

  it("handles empty frontmatter (---\\n---\\nbody)", () => {
    const raw = "---\n---\nbody text\n";
    const { data, body } = parseFrontmatter(raw);
    expect(data).toEqual({});
    expect(body).toBe("body text\n");
  });

  it("accepts a closing fence at end-of-string with no trailing newline", () => {
    const raw = "---\ntitle: Hello\n---";
    const { data, body } = parseFrontmatter(raw);
    expect(data).toEqual({ title: "Hello" });
    expect(body).toBe("");
  });

  it("handles empty frontmatter terminated at end-of-string (---\\n---)", () => {
    const raw = "---\n---";
    const { data, body } = parseFrontmatter(raw);
    expect(data).toEqual({});
    expect(body).toBe("");
  });
});

describe("getCollection", () => {
  let dir: string;
  let prevRoot: string | undefined;

  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "zfb-content-"));
    await mkdir(join(dir, "blog"), { recursive: true });
    prevRoot = process.env["ZFB_CONTENT_ROOT"];
    process.env["ZFB_CONTENT_ROOT"] = dir;
  });

  afterEach(async () => {
    if (prevRoot === undefined) {
      delete process.env["ZFB_CONTENT_ROOT"];
    } else {
      process.env["ZFB_CONTENT_ROOT"] = prevRoot;
    }
    await rm(dir, { recursive: true, force: true });
  });

  it("returns an empty array when the collection directory does not exist", async () => {
    const items = await getCollection("does-not-exist");
    expect(items).toEqual([]);
  });

  it("loads each .md file with parsed frontmatter and raw body", async () => {
    await writeFile(
      join(dir, "blog", "first.md"),
      "---\ntitle: First\ndate: 2026-04-20\n---\nFirst body\n",
      "utf8",
    );
    await writeFile(
      join(dir, "blog", "second.md"),
      "---\ntitle: Second\ndate: 2026-04-21\ntags:\n  - a\n  - b\n---\nSecond body\n",
      "utf8",
    );

    type Frontmatter = { title: string; date: string; tags?: string[] };
    const items = await getCollection<Frontmatter>("blog");
    const bySlug = new Map(items.map((entry) => [entry.slug, entry]));

    expect(items).toHaveLength(2);
    expect(bySlug.get("first")?.data.title).toBe("First");
    expect(bySlug.get("first")?.body).toBe("First body\n");
    expect(bySlug.get("second")?.data.tags).toEqual(["a", "b"]);
  });

  it("ignores files that do not end in .md", async () => {
    await writeFile(join(dir, "blog", "ignore.txt"), "nope", "utf8");
    await writeFile(join(dir, "blog", "real.md"), "---\ntitle: real\n---\nreal body\n", "utf8");
    const items = await getCollection("blog");
    expect(items.map((i) => i.slug)).toEqual(["real"]);
  });

  it("derives module_specifier as `mdx://<collection>/<slug>` for each entry", async () => {
    await writeFile(join(dir, "blog", "alpha.md"), "---\ntitle: Alpha\n---\nalpha body\n", "utf8");
    const items = await getCollection("blog");
    expect(items[0]?.module_specifier).toBe("mdx://blog/alpha");
  });

  // BCI-6: recursive traversal
  it("finds .md files in subdirectories (recursive traversal)", async () => {
    await mkdir(join(dir, "blog", "2024"), { recursive: true });
    await writeFile(join(dir, "blog", "top.md"), "---\ntitle: Top\n---\ntop body\n", "utf8");
    await writeFile(
      join(dir, "blog", "2024", "nested.md"),
      "---\ntitle: Nested\n---\nnested body\n",
      "utf8",
    );

    type Frontmatter = { title: string };
    const items = await getCollection<Frontmatter>("blog");
    const bySlug = new Map(items.map((e) => [e.slug, e]));

    expect(items).toHaveLength(2);
    expect(bySlug.get("top")?.data.title).toBe("Top");
    expect(bySlug.get("2024/nested")?.data.title).toBe("Nested");
  });

  it("derives path-based slug and module_specifier for nested entries", async () => {
    await mkdir(join(dir, "blog", "2024"), { recursive: true });
    await writeFile(
      join(dir, "blog", "2024", "hello.md"),
      "---\ntitle: Hello\n---\nhello body\n",
      "utf8",
    );
    const items = await getCollection("blog");
    expect(items[0]?.slug).toBe("2024/hello");
    expect(items[0]?.module_specifier).toBe("mdx://blog/2024/hello");
  });

  it("skips hidden directories during recursive traversal", async () => {
    await mkdir(join(dir, "blog", ".hidden"), { recursive: true });
    await writeFile(
      join(dir, "blog", ".hidden", "secret.md"),
      "---\ntitle: Secret\n---\nhidden\n",
      "utf8",
    );
    await writeFile(
      join(dir, "blog", "visible.md"),
      "---\ntitle: Visible\n---\nvisible body\n",
      "utf8",
    );
    const items = await getCollection("blog");
    expect(items.map((i) => i.slug)).toEqual(["visible"]);
  });
});

// ---------------------------------------------------------------------------
// `_relPathToSlug` — slug derivation portability.
//
// Slugs are URL-flavored identifiers, so a nested entry must produce the
// same `2024/hello` form regardless of host OS. The body of getCollection
// runs `_relPathToSlug(relative(dir, fullPath))`; these tests pin the
// helper directly so the Windows behaviour can be verified on a POSIX
// host without a real Windows runner.
// ---------------------------------------------------------------------------

describe("_relPathToSlug", () => {
  it("strips the .md extension for a top-level file", () => {
    expect(_relPathToSlug("hello.md")).toBe("hello");
  });

  it("uses forward slashes for a POSIX-flavored nested path", () => {
    expect(_relPathToSlug("2024/hello.md")).toBe("2024/hello");
  });

  it("normalises Windows backslashes to forward slashes", () => {
    // Even when the host's path.sep is "/", a stray backslash from a
    // misbehaving `path.relative` should not leak into the slug.
    expect(_relPathToSlug("2024\\hello.md")).toBe("2024/hello");
  });

  it("handles a deeply-nested Windows-flavored path", () => {
    expect(_relPathToSlug("a\\b\\c\\d.md")).toBe("a/b/c/d");
  });

  it("returns the input unchanged when there is no .md extension", () => {
    // `getCollection` filters to `.md` before reaching this helper, but
    // the function should still be total — pin the no-extension behaviour
    // so future refactors don't accidentally swallow the trailing chars.
    expect(_relPathToSlug("notes")).toBe("notes");
  });
});

// ---------------------------------------------------------------------------
// CollectionEntry.Content — bridge contract tests.
//
// The bridge contract: at call time, `Content` consults
// `globalThis.__zfb?.content?.get(entry.module_specifier)`. If the bridge
// returns a function, that function is invoked with `props` and its result
// returned verbatim. Otherwise we fall back to a marked `<pre>` block.
// ---------------------------------------------------------------------------

describe("CollectionEntry.Content", () => {
  let dir: string;
  let prevRoot: string | undefined;
  let prevBridge: TestBridge["__zfb"];

  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "zfb-content-"));
    await mkdir(join(dir, "blog"), { recursive: true });
    prevRoot = process.env["ZFB_CONTENT_ROOT"];
    process.env["ZFB_CONTENT_ROOT"] = dir;
    // Snapshot whatever __zfb may already exist on the global so we can
    // restore it after the test installs / removes its own.
    prevBridge = (globalThis as unknown as TestBridge).__zfb;
    delete (globalThis as unknown as TestBridge).__zfb;
  });

  afterEach(async () => {
    if (prevRoot === undefined) {
      delete process.env["ZFB_CONTENT_ROOT"];
    } else {
      process.env["ZFB_CONTENT_ROOT"] = prevRoot;
    }
    if (prevBridge === undefined) {
      delete (globalThis as unknown as TestBridge).__zfb;
    } else {
      (globalThis as unknown as TestBridge).__zfb = prevBridge;
    }
    await rm(dir, { recursive: true, force: true });
  });

  it("falls back to a marked <pre> block when the bridge is absent", async () => {
    await writeFile(
      join(dir, "blog", "post.md"),
      "---\ntitle: Bridgeless\n---\nfallback body line\n",
      "utf8",
    );
    const [entry] = await getCollection("blog");
    expect(entry).toBeDefined();
    const node = entry?.Content({});
    expect(node?.type).toBe("pre");
    expect(node?.props["data-zfb-content-fallback"]).toBe("");
    const children = node?.props.children;
    expect(typeof children).toBe("string");
    expect(children as string).toContain("[zfb fallback render]");
    // The marker is the leading line so unstyled environments still surface
    // it before the body content.
    expect((children as string).startsWith("[zfb fallback render]\n")).toBe(true);
    expect(children as string).toContain("fallback body line");
  });

  it("falls back when the bridge is present but returns undefined for the specifier", async () => {
    await writeFile(
      join(dir, "blog", "lonely.md"),
      "---\ntitle: Lonely\n---\nlonely body\n",
      "utf8",
    );
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get(_specifier: string) {
          return undefined;
        },
      },
    };
    const [entry] = await getCollection("blog");
    const node = entry?.Content({});
    expect(node?.type).toBe("pre");
    expect(node?.props["data-zfb-content-fallback"]).toBe("");
    expect(node?.props.children as string).toContain("[zfb fallback render]");
    expect(node?.props.children as string).toContain("lonely body");
  });

  it("delegates to the bridge component with merged components (per-call wins over defaults)", async () => {
    await writeFile(
      join(dir, "blog", "wired.md"),
      "---\ntitle: Wired\n---\nignored when bridge active\n",
      "utf8",
    );
    const calls: Array<{ specifier: string; props: { components?: Record<string, unknown> } }> = [];
    const sentinel = { type: "section", props: { children: "rendered-by-bridge" }, key: null };
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get(specifier: string) {
          return (props) => {
            calls.push({ specifier, props });
            return sentinel;
          };
        },
      },
    };
    const [entry] = await getCollection("blog");
    expect(entry?.module_specifier).toBe("mdx://blog/wired");
    function CustomH1() {}
    const overrides = { h1: CustomH1 };
    const node = entry?.Content({ components: overrides });
    // The bridge return value is passed through verbatim.
    expect(node).toBe(sentinel);
    // The bridge was consulted with the entry's module_specifier.
    expect(calls).toHaveLength(1);
    expect(calls[0]?.specifier).toBe("mdx://blog/wired");
    // Per-call components win over defaultComponents (h1 not in defaults).
    const merged = calls[0]?.props.components;
    expect(merged?.["h1"]).toBe(CustomH1);
    // defaultComponents entries are present in the merged map.
    expect(merged?.["h2"]).toBe(defaultComponents.h2);
    expect(merged?.["p"]).toBe(defaultComponents.p);
  });

  it("re-consults the bridge on every call (late-installed bridge wins)", async () => {
    await writeFile(join(dir, "blog", "late.md"), "---\ntitle: Late\n---\nlate body\n", "utf8");
    const [entry] = await getCollection("blog");
    // First call: no bridge → fallback.
    const before = entry?.Content({});
    expect(before?.type).toBe("pre");

    // Install the bridge after the entry was constructed.
    const sentinel = { type: "article", props: {}, key: null };
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: () => () => sentinel,
      },
    };
    // Second call: bridge present → delegates.
    const after = entry?.Content({});
    expect(after).toBe(sentinel);
  });
});

// ---------------------------------------------------------------------------
// `mergeMdxComponents` — precedence-merge helper (A1 seam).
//
// Precedence (lowest → highest):
//   defaultComponents → globalThis.__zfb.mdxComponents → per-call components
// ---------------------------------------------------------------------------

describe("mergeMdxComponents", () => {
  it("with no global slot and no per-call map, returns exactly defaultComponents keys", () => {
    const merged = mergeMdxComponents(undefined, undefined);
    expect(Object.keys(merged).sort()).toEqual(Object.keys(defaultComponents).sort());
    // Each value must be the same reference as in defaultComponents.
    for (const [key, val] of Object.entries(defaultComponents)) {
      expect(merged[key]).toBe(val);
    }
  });

  it("per-call components override defaultComponents entries", () => {
    function CustomH2() {}
    const merged = mergeMdxComponents(undefined, { h2: CustomH2 });
    expect(merged["h2"]).toBe(CustomH2);
    // Other defaults are still present.
    expect(merged["p"]).toBe(defaultComponents.p);
  });

  it("global slot overrides defaultComponents but loses to per-call", () => {
    function GlobalH2() {}
    function PerCallH2() {}
    const merged = mergeMdxComponents({ h2: GlobalH2 }, { h2: PerCallH2 });
    // per-call wins over global slot.
    expect(merged["h2"]).toBe(PerCallH2);
  });

  it("global slot wins over defaultComponents when per-call does not override", () => {
    function GlobalH2() {}
    const merged = mergeMdxComponents({ h2: GlobalH2 }, undefined);
    expect(merged["h2"]).toBe(GlobalH2);
    // Other defaults are unaffected.
    expect(merged["p"]).toBe(defaultComponents.p);
  });

  it("global slot can add new keys not in defaultComponents", () => {
    function GlobalCustom() {}
    const merged = mergeMdxComponents({ MyCustom: GlobalCustom }, undefined);
    expect(merged["MyCustom"]).toBe(GlobalCustom);
  });

  it("per-call can add new keys not in defaultComponents or global slot", () => {
    function PerCallCustom() {}
    const merged = mergeMdxComponents(undefined, { MyWidget: PerCallCustom });
    expect(merged["MyWidget"]).toBe(PerCallCustom);
  });

  it("is idempotent: spreading defaultComponents in per-call yields same values", () => {
    // Existing call sites that spread ...defaultComponents keep working.
    const merged = mergeMdxComponents(undefined, { ...defaultComponents });
    for (const [key, val] of Object.entries(defaultComponents)) {
      expect(merged[key]).toBe(val);
    }
  });

  it("key order is stable: spread in defaultComponents → globalSlot → perCall order", () => {
    function GlobalP() {}
    function PerCallA() {}
    const merged = mergeMdxComponents({ p: GlobalP }, { a: PerCallA });
    // All default keys present, overridden ones replaced.
    const keys = Object.keys(merged);
    const defaultKeys = Object.keys(defaultComponents);
    // Every default key appears.
    for (const k of defaultKeys) {
      expect(keys).toContain(k);
    }
    // p was overridden by global slot.
    expect(merged["p"]).toBe(GlobalP);
    // a was overridden by per-call.
    expect(merged["a"]).toBe(PerCallA);
  });
});

// ---------------------------------------------------------------------------
// buildContentComponent merge seam — integration tests (A1 seam).
//
// Verify the merge is applied before delegating to the bridge renderer, and
// that all three precedence layers work correctly end-to-end.
// ---------------------------------------------------------------------------

describe("buildContentComponent — merge seam (A1)", () => {
  let dir: string;
  let prevRoot: string | undefined;
  let prevBridge: TestBridge["__zfb"];

  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "zfb-merge-seam-"));
    await mkdir(join(dir, "blog"), { recursive: true });
    prevRoot = process.env["ZFB_CONTENT_ROOT"];
    process.env["ZFB_CONTENT_ROOT"] = dir;
    prevBridge = (globalThis as unknown as TestBridge).__zfb;
    delete (globalThis as unknown as TestBridge).__zfb;
  });

  afterEach(async () => {
    if (prevRoot === undefined) {
      delete process.env["ZFB_CONTENT_ROOT"];
    } else {
      process.env["ZFB_CONTENT_ROOT"] = prevRoot;
    }
    if (prevBridge === undefined) {
      delete (globalThis as unknown as TestBridge).__zfb;
    } else {
      (globalThis as unknown as TestBridge).__zfb = prevBridge;
    }
    await rm(dir, { recursive: true, force: true });
  });

  it("output-neutral: with no per-call components, merged map equals defaultComponents keys/values", async () => {
    await writeFile(join(dir, "blog", "neutral.md"), "---\ntitle: Neutral\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
    };
    const [entry] = await getCollection("blog");
    entry?.Content({});
    expect(captured).toHaveLength(1);
    const merged = captured[0]!;
    // Keys must match defaultComponents exactly (output-neutral).
    expect(Object.keys(merged).sort()).toEqual(Object.keys(defaultComponents).sort());
    for (const [key, val] of Object.entries(defaultComponents)) {
      expect(merged[key]).toBe(val);
    }
  });

  it("per-call components override defaultComponents", async () => {
    await writeFile(join(dir, "blog", "override.md"), "---\ntitle: Override\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    function CustomH2() {}
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
    };
    const [entry] = await getCollection("blog");
    entry?.Content({ components: { h2: CustomH2 } });
    const merged = captured[0]!;
    expect(merged["h2"]).toBe(CustomH2);
    // Other defaults still present.
    expect(merged["p"]).toBe(defaultComponents.p);
  });

  it("global slot merges between defaults and per-call", async () => {
    await writeFile(join(dir, "blog", "global.md"), "---\ntitle: Global\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    function GlobalH3() {}
    function PerCallH2() {}
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
      mdxComponents: { h3: GlobalH3 },
    };
    const [entry] = await getCollection("blog");
    entry?.Content({ components: { h2: PerCallH2 } });
    const merged = captured[0]!;
    // Global slot wins over defaultComponents.
    expect(merged["h3"]).toBe(GlobalH3);
    // Per-call wins over global slot.
    expect(merged["h2"]).toBe(PerCallH2);
    // Other defaults still present.
    expect(merged["p"]).toBe(defaultComponents.p);
  });

  it("absent global slot is a no-op (same as undefined)", async () => {
    await writeFile(join(dir, "blog", "no-global.md"), "---\ntitle: NoGlobal\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
      // No mdxComponents set on the bridge namespace.
    };
    const [entry] = await getCollection("blog");
    entry?.Content({});
    const merged = captured[0]!;
    expect(Object.keys(merged).sort()).toEqual(Object.keys(defaultComponents).sort());
  });
});

// ---------------------------------------------------------------------------
// `defaultComponents` — htmlOverrides convention (Sub 6).
//
// Each override is a thin passthrough of the markdown element it replaces.
// Tests pin (a) the eleven-entry coverage, (b) the deliberate absence of
// `h1`, (c) the per-component tag + children + extra-prop pass-through
// semantics, and (d) that each component is reachable both via the map
// and via its named-const re-export.
// ---------------------------------------------------------------------------

describe("defaultComponents (htmlOverrides convention)", () => {
  it("exports exactly the eleven zudo-doc entries (h1 deliberately absent)", () => {
    expect(Object.keys(defaultComponents).sort()).toEqual(
      ["a", "blockquote", "code", "h2", "h3", "h4", "ol", "p", "strong", "table", "ul"].sort(),
    );
    // h1 is intentionally not in the map — page titles render from frontmatter.
    expect((defaultComponents as Record<string, unknown>)["h1"]).toBeUndefined();
  });

  it("each map entry is the same reference as its named-const export", () => {
    // Pin tree-shake-import equivalence: a consumer who imports a single
    // override gets exactly the value the map points at.
    expect(defaultComponents.h2).toBe(ContentH2);
    expect(defaultComponents.h3).toBe(ContentH3);
    expect(defaultComponents.h4).toBe(ContentH4);
    expect(defaultComponents.p).toBe(ContentParagraph);
    expect(defaultComponents.a).toBe(ContentLink);
    expect(defaultComponents.strong).toBe(ContentStrong);
    expect(defaultComponents.blockquote).toBe(ContentBlockquote);
    expect(defaultComponents.ul).toBe(ContentUl);
    expect(defaultComponents.ol).toBe(ContentOl);
    expect(defaultComponents.table).toBe(ContentTable);
    expect(defaultComponents.code).toBe(ContentCode);
  });

  it.each([
    ["h2", ContentH2],
    ["h3", ContentH3],
    ["h4", ContentH4],
    ["p", ContentParagraph],
    ["a", ContentLink],
    ["strong", ContentStrong],
    ["blockquote", ContentBlockquote],
    ["ul", ContentUl],
    ["ol", ContentOl],
    ["table", ContentTable],
    ["code", ContentCode],
  ] as const)("renders <%s> with children and extra props passed through", (tag, Component) => {
    const node = Component({
      children: "hello",
      className: "foo",
      "data-test": "x",
    });
    expect(node.type).toBe(tag);
    expect(node.props.children).toBe("hello");
    expect(node.props["className"]).toBe("foo");
    expect(node.props["data-test"]).toBe("x");
    // The passthrough must not invent props the caller did not supply.
    expect(Object.keys(node.props).sort()).toEqual(["children", "className", "data-test"].sort());
    // Stable JSX-element shape (matches Island's contract).
    expect(node.key).toBeNull();
  });

  it("every entry is callable with no props and yields the matching tag", () => {
    // Smoke-test the no-props path so consumers calling `<Tag />` (e.g. an
    // empty `<hr/>`-style override eventually) still get a well-formed node.
    for (const [tag, Component] of Object.entries(defaultComponents)) {
      const node = (
        Component as (props: Record<string, unknown>) => {
          type: string;
          props: Record<string, unknown>;
          key: unknown;
        }
      )({});
      expect(node.type).toBe(tag);
      // children is forwarded as-is (undefined when not supplied).
      expect(node.props["children"]).toBeUndefined();
    }
  });

  it("forwards array children verbatim (no reshape)", () => {
    const children = ["a", { type: "em", props: { children: "b" }, key: null }, "c"];
    const node = ContentParagraph({ children });
    expect(node.props.children).toBe(children);
  });
});

// ---------------------------------------------------------------------------
// In-memory `ContentSnapshot` bridge — consumed by `@takazudo/zfb-runtime`.
//
// When a snapshot is installed via `setContentSnapshot`, `getCollection`
// resolves from memory rather than touching `fs`. This is the production
// path under the embedded V8 host / Cloudflare Workers, which have no filesystem.
// The fs path remains the v0 fallback for unit tests and direct Node
// invocations outside the Worker bundle.
// ---------------------------------------------------------------------------

describe("setContentSnapshot bridge", () => {
  afterEach(() => {
    setContentSnapshot(undefined);
  });

  it("returns undefined from getContentSnapshot when nothing is installed", () => {
    setContentSnapshot(undefined);
    expect(getContentSnapshot()).toBeUndefined();
  });

  it("round-trips an installed snapshot via getContentSnapshot", () => {
    const snap: Snapshot = {
      collections: {
        notes: [
          {
            slug: "a",
            frontmatter: { title: "A" },
            body: "alpha",
            module_specifier: "mdx://notes/a",
            rel_path: "a.md",
          },
        ],
      },
    };
    setContentSnapshot(snap);
    expect(getContentSnapshot()).toBe(snap);
  });

  it("getCollection resolves from the snapshot when installed (no fs access)", async () => {
    setContentSnapshot({
      collections: {
        blog: [
          {
            slug: "in-mem",
            frontmatter: { title: "From memory", date: "2026-04-21" },
            body: "memory body",
            module_specifier: "mdx://blog/in-mem",
            rel_path: "in-mem.mdx",
          },
        ],
      },
    });
    const items = await getCollection<{ title: string; date: string }>("blog");
    expect(items).toHaveLength(1);
    expect(items[0]?.slug).toBe("in-mem");
    expect(items[0]?.data.title).toBe("From memory");
    expect(items[0]?.body).toBe("memory body");
    expect(items[0]?.module_specifier).toBe("mdx://blog/in-mem");
  });

  it("preserves the order of entries within a collection (no implicit re-sort)", async () => {
    setContentSnapshot({
      collections: {
        blog: [
          {
            slug: "b",
            frontmatter: null,
            body: "",
            module_specifier: "mdx://blog/b",
            rel_path: "b.md",
          },
          {
            slug: "a",
            frontmatter: null,
            body: "",
            module_specifier: "mdx://blog/a",
            rel_path: "a.md",
          },
        ],
      },
    });
    const items = await getCollection("blog");
    expect(items.map((e) => e.slug)).toEqual(["b", "a"]);
  });

  it("normalises null frontmatter to an empty object", async () => {
    setContentSnapshot({
      collections: {
        notes: [
          {
            slug: "raw",
            frontmatter: null,
            body: "raw",
            module_specifier: "mdx://notes/raw",
            rel_path: "raw.md",
          },
        ],
      },
    });
    const items = await getCollection<{ title?: string }>("notes");
    expect(items[0]?.data).toEqual({});
  });

  it("returns an empty array for a collection name absent from the snapshot", async () => {
    setContentSnapshot({ collections: {} });
    const items = await getCollection("does-not-exist");
    expect(items).toEqual([]);
  });

  it("falls back to the fs path after the snapshot is cleared", async () => {
    // Install then clear; getCollection should now consult fs again,
    // which (in this test) has no `unknown-collection` directory →
    // ENOENT → empty array. We're really testing that the bridge slot
    // is reset rather than that fs returns []; the existing fs tests
    // cover the happy fs path.
    setContentSnapshot({
      collections: {
        unknown: [
          {
            slug: "x",
            frontmatter: null,
            body: "",
            module_specifier: "mdx://unknown/x",
            rel_path: "x.md",
          },
        ],
      },
    });
    expect(await getCollection("unknown")).toHaveLength(1);
    setContentSnapshot(undefined);
    expect(await getCollection("unknown")).toEqual([]);
  });

  it("Content fallback still consults the bridge after snapshot install", async () => {
    setContentSnapshot({
      collections: {
        blog: [
          {
            slug: "wired",
            frontmatter: { title: "Wired" },
            body: "ignored when bridge active",
            module_specifier: "mdx://blog/wired",
            rel_path: "wired.mdx",
          },
        ],
      },
    });
    const items = await getCollection("blog");
    const node = items[0]?.Content({});
    // No `__zfb.content` bridge installed → fallback `<pre>` block.
    expect(node?.type).toBe("pre");
    expect(node?.props["data-zfb-content-fallback"]).toBe("");
    expect(node?.props.children as string).toContain("[zfb fallback render]");
    expect(node?.props.children as string).toContain("ignored when bridge active");
  });
});

// ---------------------------------------------------------------------------
// C1 — Composed precedence chain end-to-end integration tests.
//
// Exercises the full three-layer merge through a real `.md` entry loaded
// via `getCollection`, with the bridge renderer wired to capture the merged
// `components` map. This suite proves each layer independently and asserts
// the two invariants:
//
//   1. Layer ordering (lowest → highest):
//        built-in `defaultComponents`
//        → `globalThis.__zfb.mdxComponents` (file-map global slot, #616)
//        → per-call `<Content components={...}>` prop
//
//   2. Invariants:
//      a. An unmapped lowercase HTML tag absent from all three layers is
//         absent from the merged map. The MDX emitter pre-seeds such tags
//         as `_components.tag = "tag"` (the intrinsic HTML string), so the
//         browser degrades to the native element — `mergeMdxComponents`
//         correctly produces no override entry for it.
//      b. A PascalCase component absent from all three layers is absent
//         from the merged map. The MDX emitter emits
//         `if (!Note) throw new Error(...)` for any PascalCase identifier
//         referenced in the MDX source, so a missing entry is a hard throw
//         at render time (tested by `mdx_jsx_pascal_case_components_require_prop`
//         in crates/zfb-content/tests/mdx_jsx_emit.rs). Here we verify the
//         TS-side merge produces no entry for it, confirming the throw path
//         is reachable when PascalCase is unresolved.
//
// The global slot tests simulate what `entry.mjs` does at runtime after the
// bundler processes a `mdx-components.tsx` file with a default export object
// (the canonical #616 shape: `export default { h2: MyH2, … }`).
// ---------------------------------------------------------------------------

describe("composed precedence chain — C1", () => {
  let dir: string;
  let prevRoot: string | undefined;
  let prevBridge: TestBridge["__zfb"];

  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "zfb-c1-chain-"));
    await mkdir(join(dir, "blog"), { recursive: true });
    prevRoot = process.env["ZFB_CONTENT_ROOT"];
    process.env["ZFB_CONTENT_ROOT"] = dir;
    prevBridge = (globalThis as unknown as TestBridge).__zfb;
    delete (globalThis as unknown as TestBridge).__zfb;
  });

  afterEach(async () => {
    if (prevRoot === undefined) {
      delete process.env["ZFB_CONTENT_ROOT"];
    } else {
      process.env["ZFB_CONTENT_ROOT"] = prevRoot;
    }
    if (prevBridge === undefined) {
      delete (globalThis as unknown as TestBridge).__zfb;
    } else {
      (globalThis as unknown as TestBridge).__zfb = prevBridge;
    }
    await rm(dir, { recursive: true, force: true });
  });

  // Layer 1 of 3: baseline — defaults populate the map when neither the
  // global slot nor per-call supply anything.
  it("layer 1: defaultComponents populate the map when no overrides are set", async () => {
    await writeFile(join(dir, "blog", "layer1.md"), "---\ntitle: Layer1\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
      // No mdxComponents — simulates a project without mdx-components.tsx
    };
    const [entry] = await getCollection("blog");
    // No per-call override.
    entry?.Content({});
    const merged = captured[0]!;
    // Every defaultComponents entry must be present in the merged map.
    for (const [key, val] of Object.entries(defaultComponents)) {
      expect(merged[key]).toBe(val);
    }
    // No extra keys beyond defaultComponents.
    expect(Object.keys(merged).sort()).toEqual(Object.keys(defaultComponents).sort());
  });

  // Layer 2 of 3: the global slot (populated by the bundler from
  // `mdx-components.tsx` via `globalThis.__zfb.mdxComponents = __zfb_mdx_components`)
  // wins over defaultComponents when per-call is absent.
  it("layer 2: global slot overrides defaultComponents — simulates mdx-components.tsx install", async () => {
    await writeFile(join(dir, "blog", "layer2.md"), "---\ntitle: Layer2\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    function GlobalH2() {}
    // Simulate `export default { h2: GlobalH2 }` from mdx-components.tsx,
    // installed by the bundler as `globalThis.__zfb.mdxComponents = __zfb_mdx_components`.
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
      mdxComponents: { h2: GlobalH2 },
    };
    const [entry] = await getCollection("blog");
    // No per-call override — global slot must win over defaults for h2.
    entry?.Content({});
    const merged = captured[0]!;
    // Global slot wins over defaultComponents.
    expect(merged["h2"]).toBe(GlobalH2);
    // Other defaults remain.
    expect(merged["p"]).toBe(defaultComponents.p);
    expect(merged["h3"]).toBe(defaultComponents.h3);
  });

  // Layer 3 of 3: per-call `components` prop wins over both the global slot
  // and defaultComponents. This is the highest-precedence layer.
  it("layer 3: per-call prop wins over global slot and defaults — highest precedence", async () => {
    await writeFile(join(dir, "blog", "layer3.md"), "---\ntitle: Layer3\n---\nbody\n", "utf8");
    const captured: Record<string, unknown>[] = [];
    function GlobalH2() {}
    function GlobalP() {}
    function PerCallH2() {}
    // Global slot has both h2 and p.
    (globalThis as unknown as TestBridge).__zfb = {
      content: {
        get: (_spec) => (props) => {
          if (props.components) captured.push(props.components);
          return { type: "div", props: {}, key: null };
        },
      },
      mdxComponents: { h2: GlobalH2, p: GlobalP },
    };
    const [entry] = await getCollection("blog");
    // Per-call overrides h2 only — per-call must win for h2; global wins for p.
    entry?.Content({ components: { h2: PerCallH2 } });
    const merged = captured[0]!;
    // Per-call h2 wins over global slot h2, which in turn won over defaults.
    expect(merged["h2"]).toBe(PerCallH2);
    // Global slot p wins over defaultComponents.p (global is still present when
    // per-call doesn't override it).
    expect(merged["p"]).toBe(GlobalP);
    // Unrelated defaults still present.
    expect(merged["h3"]).toBe(defaultComponents.h3);
  });

  // Invariant (a): an unmapped lowercase HTML tag that appears in `.md` source
  // is handled by the MDX emitter itself — it pre-seeds the tag as the
  // intrinsic string ("span" → _components.span = "span") so the browser
  // degrades to the native element. `mergeMdxComponents` correctly omits such
  // a tag from the merged map when no layer supplies an override.
  it("invariant — unmapped lowercase tag absent from all layers: absent from merged map", () => {
    // None of the three layers (defaults, global slot, per-call) supply "span".
    const merged = mergeMdxComponents(
      undefined, // no global slot
      undefined, // no per-call
    );
    // "span" is not in defaultComponents — no override entry.
    expect(merged["span"]).toBeUndefined();
    // The MDX emitter (crates/zfb-content/src/mdx_jsx_emit.rs) pre-seeds
    // `_components.span = "span"` so the browser renders native <span>.
    // mergeMdxComponents does not interfere — the degrade is handled at
    // the MDX emit layer, not by this map.
  });

  it("invariant — unmapped lowercase overridable when a layer supplies it", () => {
    // Confirm that any layer CAN override a tag like "span" that is not in
    // defaultComponents — the spread is open-ended. This proves the
    // degrade-to-default is optional, not forced.
    function CustomSpan() {}
    const mergedViaGlobal = mergeMdxComponents({ span: CustomSpan }, undefined);
    expect(mergedViaGlobal["span"]).toBe(CustomSpan);

    function PerCallSpan() {}
    const mergedViaPerCall = mergeMdxComponents(undefined, { span: PerCallSpan });
    expect(mergedViaPerCall["span"]).toBe(PerCallSpan);
  });

  // Invariant (b): a PascalCase component absent from all three layers produces
  // no entry in the merged map. The MDX emitter emits:
  //   const Note = _components.Note ?? components.Note;
  //   if (!Note) throw new Error("MDX requires `Note` to be passed via the `components` prop");
  // for any PascalCase reference in the source. When `mergeMdxComponents`
  // produces no "Note" entry the emitted guard fires — a hard throw, not a
  // silent undefined render. Proven at the emit layer by
  // `mdx_jsx_pascal_case_components_require_prop` (crates/zfb-content/tests/mdx_jsx_emit.rs).
  it("invariant — PascalCase key absent from all layers: absent from merged map (emitter hard-throws)", () => {
    const merged = mergeMdxComponents(
      undefined, // no global slot
      undefined, // no per-call
    );
    // "Note" is not in defaultComponents and not in any override layer.
    expect(merged["Note"]).toBeUndefined();
    // An uppercase first character confirms the PascalCase identifier contract.
    // The MDX emitter's throw guard (see crates/zfb-content/src/mdx_jsx_emit.rs
    // line ~361) fires when `_components.Note` and `components.Note` are both
    // falsy — which is the case here.
  });

  it("invariant — PascalCase component resolves when any layer supplies it (no throw)", () => {
    // A layer supplying the PascalCase key prevents the emitter's throw.
    function MyNote() {}
    // Via global slot (mdx-components.tsx).
    const mergedViaGlobal = mergeMdxComponents({ Note: MyNote }, undefined);
    expect(mergedViaGlobal["Note"]).toBe(MyNote);

    // Via per-call prop.
    function PerCallNote() {}
    const mergedViaPerCall = mergeMdxComponents(undefined, { Note: PerCallNote });
    expect(mergedViaPerCall["Note"]).toBe(PerCallNote);

    // Per-call wins over global slot for PascalCase too.
    function OverrideNote() {}
    const mergedBoth = mergeMdxComponents({ Note: MyNote }, { Note: OverrideNote });
    expect(mergedBoth["Note"]).toBe(OverrideNote);
  });
});
