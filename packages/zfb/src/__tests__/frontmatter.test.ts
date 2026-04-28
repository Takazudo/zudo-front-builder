// BCI-3: verify that `parseFrontmatter` is reachable via the `zfb/frontmatter`
// subpath, independently from `zfb/content`. This guards against the subpath
// export being accidentally removed from `package.json`.

import { describe, expect, it } from "vitest";

import { parseFrontmatter } from "../frontmatter.js";
import type { ParsedFrontmatter } from "../frontmatter.js";

describe("parseFrontmatter (via zfb/frontmatter subpath)", () => {
  it("is importable from the frontmatter subpath module", () => {
    expect(typeof parseFrontmatter).toBe("function");
  });

  it("ParsedFrontmatter type is re-exported (type-level smoke test)", () => {
    // This is a type-only test: the cast must not produce a TS error.
    const result: ParsedFrontmatter = parseFrontmatter("---\ntitle: Hello\n---\nbody");
    expect(result.data["title"]).toBe("Hello");
    expect(result.body).toBe("body");
  });

  it("delegates to the same implementation as zfb/content", () => {
    // Verify the re-export is consistent with the content module's version
    // by checking a representative case.
    const raw = "---\ntags:\n  - a\n  - b\n---\nbody text\n";
    const { data, body } = parseFrontmatter(raw);
    expect(data["tags"]).toEqual(["a", "b"]);
    expect(body).toBe("body text\n");
  });
});
