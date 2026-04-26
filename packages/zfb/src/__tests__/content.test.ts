import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { getCollection, parseFrontmatter } from "../content.js";

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
});
