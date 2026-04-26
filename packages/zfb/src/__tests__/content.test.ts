import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
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
  parseFrontmatter,
} from "../content.js";

/**
 * Test-only handle on the `__zfb` bridge namespace. Mirrors the ambient
 * declaration the production renderer installs, narrowed to what these
 * tests touch (the `content.get(specifier)` lookup).
 */
type TestBridge = {
  __zfb?: {
    content?: {
      get(
        specifier: string,
      ): ((props: { components?: Record<string, unknown> }) => unknown) | undefined;
    };
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

  it("delegates to the bridge component when present, forwarding props verbatim", async () => {
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
    const overrides = { h1: function CustomH1() {} };
    const node = entry?.Content({ components: overrides });
    // The bridge return value is passed through verbatim.
    expect(node).toBe(sentinel);
    // The bridge was consulted with the entry's module_specifier and
    // received the same props the caller supplied.
    expect(calls).toHaveLength(1);
    expect(calls[0]?.specifier).toBe("mdx://blog/wired");
    expect(calls[0]?.props.components).toBe(overrides);
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
