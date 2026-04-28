// `zfb/frontmatter` — public subpath for the frontmatter parser.
//
// BCI-3: `parseFrontmatter` lives in this dedicated subpath so consumers
// that only need frontmatter parsing can import it without pulling in
// `zfb/content`'s Node `fs` transitive dependency chain. Workers / edge
// bundlers reading user content from a CMS or KV store can therefore
// use this parser without bringing `node:fs/promises` into their bundle.
//
// The implementation lives directly in this module — `zfb/content`
// imports from here, not the other way around. Tests pin the absence of
// transitive `node:fs*` imports so a regression that re-introduces the
// fs dependency surfaces immediately.
//
// ---------------------------------------------------------------------------
// Minimal frontmatter parser. Intentionally NOT a full YAML implementation —
// the v0 surface accepts the subset documented below. This avoids pulling
// in `gray-matter` or `js-yaml` for what is, in v0, three field types:
//
// - `key: value` scalar (quoted strings unwrapped, ISO dates kept as strings)
// - `key:` followed by a block list of `- item` lines
// - blank lines and `#` comment lines are ignored
//
// Returns `{ data: {}, body: <input> }` unchanged when no frontmatter
// fence is present, or when the opening fence has no matching closer.
// ---------------------------------------------------------------------------

const FRONTMATTER_DELIM = "---";

/** Public shape returned by [`parseFrontmatter`]. */
export type ParsedFrontmatter = {
  data: Record<string, unknown>;
  body: string;
};

/**
 * Parse a leading YAML-ish frontmatter block off a markdown document.
 *
 * **Public SDK surface.** Re-exported from `zfb/content` so consumers can
 * write their own custom content loaders without re-implementing the
 * (deliberately minimal) v0 frontmatter parser. The accepted grammar is
 * documented at the top of this module.
 *
 * Handles:
 * - empty frontmatter (`---\n---\nbody`) → `{ data: {}, body: "body" }`
 * - file ending exactly with `---` (no trailing newline) → frontmatter
 *   parsed, body is empty.
 *
 * Returns `{ data: {}, body: <input> }` unchanged when no frontmatter
 * fence is present, or when the opening fence has no matching closer.
 */
export function parseFrontmatter(raw: string): ParsedFrontmatter {
  // Strip a leading BOM and normalise line endings before splitting.
  const text = raw.replace(/^﻿/, "").replace(/\r\n/g, "\n");
  if (!text.startsWith(`${FRONTMATTER_DELIM}\n`)) {
    return { data: {}, body: text };
  }
  const headerStart = FRONTMATTER_DELIM.length + 1; // after first "---\n"

  // Search for the closing fence. Accept either `\n---\n` (frontmatter
  // followed by body) or `\n---` at the very end of the document
  // (frontmatter with no trailing newline). Start the search at
  // `headerStart - 1` so the empty-frontmatter case `---\n---\n...`
  // is detected (the `\n---` at index 3 immediately follows the opener).
  const searchFrom = headerStart - 1;
  let closeIdx = -1;
  let bodyStart = -1;
  let i = searchFrom;
  while (i <= text.length - `\n${FRONTMATTER_DELIM}`.length) {
    const candidate = text.indexOf(`\n${FRONTMATTER_DELIM}`, i);
    if (candidate === -1) break;
    const afterFence = candidate + `\n${FRONTMATTER_DELIM}`.length;
    if (afterFence === text.length) {
      // `\n---` at end-of-string — frontmatter ends here, body is empty.
      closeIdx = candidate;
      bodyStart = afterFence;
      break;
    }
    if (text.charAt(afterFence) === "\n") {
      // `\n---\n` — body starts after the trailing newline.
      closeIdx = candidate;
      bodyStart = afterFence + 1;
      break;
    }
    // `\n---` followed by more `-` (e.g. `\n----`) — keep searching.
    i = candidate + 1;
  }
  if (closeIdx === -1 || bodyStart === -1) {
    // Malformed frontmatter (no closing delimiter): treat as plain body.
    return { data: {}, body: text };
  }
  const header = text.slice(headerStart, closeIdx);
  const body = text.slice(bodyStart);
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
