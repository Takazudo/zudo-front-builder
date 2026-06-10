/**
 * Heading-anchor slug utilities — a pure-TypeScript port of the Rust
 * `slugify` / `next_slug` / `SlugAllocator` in
 * `crates/zfb-content/src/plugins/heading_links.rs`.
 *
 * These functions produce the `id` attribute values that appear on
 * `<h2>`–`<h6>` elements in the rendered HTML.  They are intentionally
 * **not** compatible with npm `github-slugger`: stripped punctuation
 * collapses to a `-` separator (`"a,b"` → `"a-b"`), and leading/trailing
 * whitespace / stripped chars emit nothing rather than a stray dash.
 *
 * ⚠️  These functions produce heading-anchor slugs (e.g. `"hello-world"`
 * for `<h2 id="hello-world">`).  They are **different** from
 * `_relPathToSlug` in `content.ts`, which converts a file-system relative
 * path into a URL slug for `CollectionEntry.slug`.
 *
 * Parity is enforced by the shared fixture at
 * `crates/zfb-content/tests/fixtures/slugify-parity.json`, consumed by
 * both a Rust integration test and the `slugify.test.ts` vitest suite.
 */

// ---------------------------------------------------------------------------
// Helpers — mirrors the Rust `is_stripped` predicate
// ---------------------------------------------------------------------------

/** Unicode General Category Cc (control characters). */
function isCc(ch: string): boolean {
  // \p{Cc} = general category "Control" (C0 + C1 blocks: U+0000–U+001F, U+007F–U+009F)
  return /^\p{Cc}$/u.test(ch);
}

/**
 * Returns true when the character should be stripped (maps to a dash
 * separator in the output).  Mirrors Rust's `is_stripped`:
 *   - Unicode Cc control characters
 *   - Explicit ASCII punctuation set: `! " # $ % & ' ( ) * + , . / : ; < = > ? @ [ \ ] ^ ` { | } ~`
 */
function isStripped(ch: string): boolean {
  if (isCc(ch)) return true;
  return (
    ch === "!" ||
    ch === '"' ||
    ch === "#" ||
    ch === "$" ||
    ch === "%" ||
    ch === "&" ||
    ch === "'" ||
    ch === "(" ||
    ch === ")" ||
    ch === "*" ||
    ch === "+" ||
    ch === "," ||
    ch === "." ||
    ch === "/" ||
    ch === ":" ||
    ch === ";" ||
    ch === "<" ||
    ch === "=" ||
    ch === ">" ||
    ch === "?" ||
    ch === "@" ||
    ch === "[" ||
    ch === "\\" ||
    ch === "]" ||
    ch === "^" ||
    ch === "`" ||
    ch === "{" ||
    ch === "|" ||
    ch === "}" ||
    ch === "~"
  );
}

/**
 * Returns true when `ch` is a Unicode whitespace character.
 *
 * Mirrors Rust `char::is_whitespace` which is `\p{White_Space}/u` — NOT
 * the JS `\s` class (`\s` wrongly matches U+FEFF BYTE ORDER MARK and
 * misses U+0085 NEXT LINE).
 */
function isWhitespace(ch: string): boolean {
  return /^\p{White_Space}$/u.test(ch);
}

// ---------------------------------------------------------------------------
// slugify
// ---------------------------------------------------------------------------

/**
 * Produce a heading-anchor slug from `input`, matching the Rust
 * `slugify()` in `crates/zfb-content/src/plugins/heading_links.rs`.
 *
 * Algorithm (per Unicode code point):
 * - whitespace or stripped punctuation → emit ONE `-` unless the last
 *   emitted character was already a `-` (leading separators are skipped
 *   because `lastDash` starts `true`).
 * - anything else → push `ch.toLowerCase()` (per-code-point, not
 *   whole-string, to avoid Greek final-sigma context collapsing).
 * - finally pop ONE trailing `-` if present.
 *
 * Divergence from npm github-slugger (intentional — parity target is
 * the Rust function, not npm):
 * - `"a,b"` → `"a-b"` (stripped punctuation emits a dash, not nothing)
 * - `"  --weird-- "` → `"--weird--"` (existing dashes/underscores pass
 *   through unchanged)
 */
export function slugify(input: string): string {
  let out = "";
  let lastDash = true;

  for (const ch of input) {
    if (isWhitespace(ch) || isStripped(ch)) {
      if (!lastDash) {
        out += "-";
        lastDash = true;
      }
    } else {
      // Per-code-point lowercase — avoids Greek final-sigma context:
      // "ΦΟΣ" → "φοσ" not "φος".
      out += ch.toLowerCase();
      lastDash = false;
    }
  }

  if (out.endsWith("-")) {
    out = out.slice(0, -1);
  }

  return out;
}

// ---------------------------------------------------------------------------
// next_slug
// ---------------------------------------------------------------------------

/**
 * Allocate the next deduplicated slug for `base` against a per-document
 * counter map.
 *
 * Mirrors Rust `next_slug`: first occurrence returns `base`, second
 * returns `base-1`, third `base-2`, …  Empty `base` short-circuits to
 * `""` without mutating the map.
 */
function nextSlug(seen: Map<string, number>, base: string): string {
  if (base === "") return "";
  const count = seen.get(base) ?? 0;
  const slug = count === 0 ? base : `${base}-${count}`;
  seen.set(base, count + 1);
  return slug;
}

// ---------------------------------------------------------------------------
// SlugAllocator
// ---------------------------------------------------------------------------

/**
 * Strategy-aware slug allocator for a single document.
 *
 * Mirrors Rust `SlugAllocator` in heading_links.rs:70-118.
 *
 * @param strategy `"flat"` (default) — per-document dedup counter, mirrors
 *   github-slugger numbering.  `"hierarchical"` — each slug is prefixed with
 *   its ancestor chain and deduped on the full candidate path.
 *
 * Depth contract: callers pass `depth` values 2–6 (h2–h6).  h1 is never
 * allocated — it is handled by the page layout, not the markdown body.
 */
export class SlugAllocator {
  private readonly strategy: "flat" | "hierarchical";
  private seen: Map<string, number>;
  /** Hierarchical ancestor stack of `[depth, finalId]` pairs. Unused in flat mode. */
  private stack: Array<[number, string]>;

  constructor(strategy: "flat" | "hierarchical" = "flat") {
    this.strategy = strategy;
    this.seen = new Map();
    this.stack = [];
  }

  /**
   * Allocate the slug for a heading of `depth` (2–6) whose slugified text
   * is `base`.  Returns `""` for an empty `base` without mutating state.
   */
  allocate(depth: number, base: string): string {
    if (this.strategy === "flat") {
      return nextSlug(this.seen, base);
    }

    // Hierarchical mode
    if (base === "") return "";

    // Pop ancestors whose depth >= this heading's depth
    let top = this.stack.at(-1);
    while (top !== undefined && top[0] >= depth) {
      this.stack.pop();
      top = this.stack.at(-1);
    }

    const candidate = top !== undefined ? `${top[1]}-${base}` : base;
    const slug = nextSlug(this.seen, candidate);
    this.stack.push([depth, slug]);
    return slug;
  }

  /**
   * Clear all per-document state (dedup counters AND the hierarchical
   * ancestor stack).  Call between documents to avoid slug counter leakage.
   */
  reset(): void {
    this.seen.clear();
    this.stack = [];
  }
}
