// `zfb/content` — minimal v0 content collection loader.
//
// Reads `*.md` files from a content collection directory, parses YAML
// frontmatter, and returns typed entries. This is a deliberately small
// stub so the basic-blog example can call `getCollection("blog")` today;
// the production path lives in `crates/zfb-content` and will replace this
// once the JS-runtime decision (ADR-001) lands and the renderer wires the
// Rust pipeline back through to user code.
//
// Scope (v0):
// - YAML-ish frontmatter only: `key: value`, plus `key:\n  - item` arrays.
//   Quoted strings are unwrapped. ISO dates stay as strings.
// - Body is the post content **after** the closing `---`, returned as raw
//   text. This is intentionally NOT pre-rendered HTML: the markdown
//   pipeline lives in the Rust crate and the JS stub does not duplicate
//   it.
// - Collection root is resolved from
//   `process.env.ZFB_CONTENT_ROOT` (set by the dev/build pipeline), or
//   `<cwd>/content` as a fallback for unit tests and direct invocation.
//
// TODO(zfb-content): swap this stub for the runtime-provided implementation
// once the content engine ships end-to-end.

import { readdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";

/**
 * Generic shape returned for one entry in a content collection. The `data`
 * field carries parsed frontmatter, typed by the caller via the generic
 * parameter.
 */
export type CollectionEntry<T = Record<string, unknown>> = {
  /** Filename without `.md` extension. Stable across runs. */
  slug: string;
  /** Parsed frontmatter. */
  data: T;
  /** Raw markdown body (frontmatter stripped). */
  body: string;
};

/**
 * Resolve the directory that holds a named content collection. Override
 * via `ZFB_CONTENT_ROOT` so tests / fixtures can point at an arbitrary
 * directory.
 */
function resolveCollectionDir(name: string): string {
  const envRoot = process.env["ZFB_CONTENT_ROOT"];
  const root = envRoot ? resolve(envRoot) : resolve(process.cwd(), "content");
  return join(root, name);
}

/**
 * Load every `*.md` file in the named collection. Files starting with `.`
 * or that lack a `.md` extension are ignored.
 *
 * @example
 *   const posts = await getCollection<{ title: string; date: string }>("blog");
 */
export async function getCollection<T = Record<string, unknown>>(
  name: string,
): Promise<CollectionEntry<T>[]> {
  const dir = resolveCollectionDir(name);
  let names: string[];
  try {
    names = await readdir(dir);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return [];
    }
    throw err;
  }
  const mdFiles = names.filter((n) => n.endsWith(".md") && !n.startsWith("."));
  const entries = await Promise.all(
    mdFiles.map(async (filename) => {
      const fullPath = join(dir, filename);
      const raw = await readFile(fullPath, "utf8");
      const { data, body } = parseFrontmatter(raw);
      const slug = filename.slice(0, -".md".length);
      return { slug, data: data as T, body };
    }),
  );
  return entries;
}

// ---------------------------------------------------------------------------
// Minimal frontmatter parser. Intentionally NOT a full YAML implementation —
// the v0 surface is documented above. This avoids pulling in `gray-matter`
// or `js-yaml` for what is, in v0, three field types.
// ---------------------------------------------------------------------------

const FRONTMATTER_DELIM = "---";

type ParsedFrontmatter = {
  data: Record<string, unknown>;
  body: string;
};

export function parseFrontmatter(raw: string): ParsedFrontmatter {
  // Strip a leading BOM and normalise line endings before splitting.
  const text = raw.replace(/^﻿/, "").replace(/\r\n/g, "\n");
  if (!text.startsWith(`${FRONTMATTER_DELIM}\n`)) {
    return { data: {}, body: text };
  }
  const closeIdx = text.indexOf(`\n${FRONTMATTER_DELIM}\n`, FRONTMATTER_DELIM.length + 1);
  if (closeIdx === -1) {
    // Malformed frontmatter (no closing delimiter): treat as plain body.
    return { data: {}, body: text };
  }
  const headerStart = FRONTMATTER_DELIM.length + 1; // after first "---\n"
  const header = text.slice(headerStart, closeIdx);
  const body = text.slice(closeIdx + `\n${FRONTMATTER_DELIM}\n`.length);
  return { data: parseFrontmatterHeader(header), body };
}

function parseFrontmatterHeader(header: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const lines = header.split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (line.trim() === "" || line.trimStart().startsWith("#")) {
      i++;
      continue;
    }
    // Top-level keys are unindented `key: value` or `key:` (then list).
    const m = /^([A-Za-z_][\w-]*)\s*:\s*(.*)$/.exec(line);
    if (!m) {
      i++;
      continue;
    }
    const key = m[1] as string;
    const inlineValue = (m[2] ?? "").trim();
    if (inlineValue === "") {
      // Possible block list.
      const list: string[] = [];
      let j = i + 1;
      while (j < lines.length) {
        const next = lines[j] ?? "";
        const itemMatch = /^\s*-\s+(.*)$/.exec(next);
        if (!itemMatch) break;
        list.push(unwrapScalar((itemMatch[1] ?? "").trim()));
        j++;
      }
      if (list.length > 0) {
        out[key] = list;
        i = j;
        continue;
      }
      // Empty value with no list — record empty string for completeness.
      out[key] = "";
      i++;
      continue;
    }
    out[key] = unwrapScalar(inlineValue);
    i++;
  }
  return out;
}

function unwrapScalar(value: string): string {
  if (value.length >= 2) {
    const first = value.charAt(0);
    const last = value.charAt(value.length - 1);
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return value.slice(1, -1);
    }
  }
  return value;
}
