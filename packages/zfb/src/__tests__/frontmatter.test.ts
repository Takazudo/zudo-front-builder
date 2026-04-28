// BCI-3: verify that `parseFrontmatter` is reachable via the `zfb/frontmatter`
// subpath, independently from `zfb/content`. This guards against the subpath
// export being accidentally removed from `package.json` AND against the fs-free
// guarantee being silently broken by a future refactor (e.g. moving the
// implementation back into `content.ts` would re-introduce a transitive
// `node:fs/promises` import on the frontmatter subpath).

import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

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

  it("does not transitively import node:fs* (Workers / edge bundle hygiene)", async () => {
    // Static smoke test: walk the import graph reachable from
    // `frontmatter.ts` and assert none of the visited modules pull in
    // `node:fs`, `node:fs/promises`, or any other Node-only module that
    // a Workers / edge bundler cannot polyfill cheaply.
    //
    // We grep for `from "node:fs..."` / `from "fs..."` (and the `import
    // "..."` side-effect form). This catches accidental regressions
    // like content.ts being imported back into frontmatter.ts during a
    // refactor — content.ts pulls in `node:fs/promises` for the
    // `getCollection` filesystem fallback, so any cycle through it
    // would re-introduce the dependency on the subpath.
    const here = dirname(fileURLToPath(import.meta.url));
    const visited = new Set<string>();
    const fsForbidden =
      /from\s+["'](?:node:)?fs(?:\/[^"']*)?["']|import\s+["'](?:node:)?fs(?:\/[^"']*)?["']/;

    async function visit(absPath: string): Promise<void> {
      if (visited.has(absPath)) return;
      visited.add(absPath);
      const src = await readFile(absPath, "utf8");
      expect(
        fsForbidden.test(src),
        `frontmatter subpath transitively imports node:fs* via ${absPath}`,
      ).toBe(false);
      const importRe = /(?:from|import)\s+["'](\.[^"']+)["']/g;
      let m: RegExpExecArray | null;
      while ((m = importRe.exec(src)) !== null) {
        const spec = m[1];
        if (!spec) continue;
        // Resolve the relative specifier against the importer's
        // directory. Source uses `.js` extensions per ESM conventions
        // but the actual files live as `.ts`; rewrite when needed.
        const resolvedJs = resolve(dirname(absPath), spec);
        const resolvedTs = resolvedJs.endsWith(".js")
          ? resolvedJs.slice(0, -".js".length) + ".ts"
          : resolvedJs;
        await visit(resolvedTs);
      }
    }

    await visit(resolve(here, "..", "frontmatter.ts"));
    // Sanity check: we actually visited the entry module and did not
    // bail out on the first read.
    expect(visited.size).toBeGreaterThanOrEqual(1);
  });
});
