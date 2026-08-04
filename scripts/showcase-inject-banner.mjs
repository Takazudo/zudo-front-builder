#!/usr/bin/env node
/**
 * showcase-inject-banner.mjs
 *
 * Deploy-time injector for the create-zfb showcase site (issue #2282, part
 * of the create-zfb-showcase epic, #2279). Given a directory of already-built
 * HTML (a `dist/` produced by `npm create zfb@latest my-site` +
 * `zfb build`) and a commit SHA, injects a banner into every `*.html` file
 * stating that the page is the unmodified output of that scaffold command,
 * naming the commit, and linking to the docs site and the GitHub repo.
 *
 * This exists so a visitor to the showcase can tell what they are looking
 * at. It runs ONLY in CI, at deploy time, against a built `dist/` — it must
 * never touch `crates/zfb/templates/**`, which is what keeps the "this is
 * exactly what you get" claim honest.
 *
 * Design constraints (see issue #2282 for the full rationale):
 *   - Plain string splice on the `<body ...>` open tag. No DOM
 *     parse/serialize round-trip (no cheerio/jsdom/node-html-parser). A
 *     full re-serialize can normalize markup, which would perturb the
 *     `element-permitted-content` result that html-validate checks in the
 *     deploy jobs, and can disturb the one island the template ships
 *     (`ThemeToggle` under `<Island when="idle">` in the header).
 *   - Idempotent: guarded on a marker comment, so running this twice over
 *     the same tree produces one banner, not two.
 *   - An HTML file with no `<body>` is skipped, left byte-unchanged. This
 *     is a deliberate decision, not an oversight.
 *
 * <ClientRouter /> tripwire: `crates/zfb/templates/` ships no
 * `<ClientRouter />` anywhere, so every navigation on the showcase today is
 * a full document load and this banner reappears on each page load. If
 * `basic-blog` ever adopts `<ClientRouter />`, the router's client-side body
 * swap will drop the banner after the first soft navigation — this script's
 * body-open-tag injection is invisible to that swap. Re-injecting on the
 * client (not just at build/deploy time) would be needed at that point.
 *
 * Usage:
 *   node scripts/showcase-inject-banner.mjs <dir> <sha>
 *
 * Uses only Node stdlib. Requires Node >= 18 (fs.readdirSync recursive
 * withFileTypes).
 */

import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { extname, join } from "node:path";

const DOCS_URL = "https://zfb.takazudomodular.com";
const REPO_URL = "https://github.com/Takazudo/zudo-front-builder";

// Wraps the injected block. injectBannerIntoHtml() checks for this exact
// string to decide whether a page already carries a banner (idempotency
// guard) — do not change it without considering already-deployed HTML.
export const BANNER_MARKER = "<!-- zfb-showcase-banner -->";

/**
 * Build the fixed banner HTML block for a given commit SHA. Styled with
 * inline `style` attributes only (no `<style>` tag, no classes) so it
 * cannot collide with the scaffold's Tailwind output — Tailwind's preflight
 * and utility classes never win against an inline style attribute.
 */
export function buildBannerHtml(sha) {
  const shortSha = sha.slice(0, 7);
  const commitUrl = `${REPO_URL}/commit/${sha}`;
  const linkStyle = "color:#93c5fd;text-decoration:underline;";
  return (
    `${BANNER_MARKER}\n` +
    `<div style="box-sizing:border-box;width:100%;padding:0.5rem 1rem;` +
    `font:14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;` +
    `background:#111827;color:#f9fafb;text-align:center;">` +
    `This page is the unmodified output of ` +
    `<code style="background:#1f2937;padding:0.1em 0.4em;border-radius:0.25em;">` +
    `npm create zfb@latest my-site</code> ` +
    `at commit <a href="${commitUrl}" style="${linkStyle}">${shortSha}</a>. ` +
    `<a href="${DOCS_URL}" style="${linkStyle}">Docs</a> &middot; ` +
    `<a href="${REPO_URL}" style="${linkStyle}">GitHub</a>` +
    `</div>\n` +
    `${BANNER_MARKER}\n`
  );
}

// Matches the opening `<body ...>` tag (attributes optional). Deliberately
// simple — this is a plain string splice, not a parser; see the
// no-DOM-round-trip constraint in the header comment above.
const BODY_OPEN_TAG = /<body\b[^>]*>/i;

/**
 * Inject the banner into a single HTML document immediately after the
 * `<body ...>` open tag. Pure string transform — no HTML parsing.
 *
 * - Idempotent: if BANNER_MARKER is already present, returns `html`
 *   unchanged (identity), so re-running never produces a second banner.
 * - If there is no `<body>` tag at all, returns `html` unchanged.
 *
 * @param {string} html
 * @param {string} sha - commit SHA to name in the banner
 * @returns {string}
 */
export function injectBannerIntoHtml(html, sha) {
  if (html.includes(BANNER_MARKER)) {
    return html;
  }
  const match = BODY_OPEN_TAG.exec(html);
  if (!match) {
    return html;
  }
  const insertAt = match.index + match[0].length;
  return html.slice(0, insertAt) + buildBannerHtml(sha) + html.slice(insertAt);
}

/**
 * Recursively list every `*.html` file under `dir`, sorted for
 * deterministic output across platforms/filesystems.
 */
function listHtmlFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...listHtmlFiles(full));
    } else if (entry.isFile() && extname(entry.name).toLowerCase() === ".html") {
      out.push(full);
    }
  }
  return out.sort();
}

/**
 * Inject the banner into every `*.html` file found recursively under
 * `dir`. Files that already carry a banner, or have no `<body>` tag, are
 * left byte-unchanged (and not rewritten to disk).
 *
 * @param {string} dir
 * @param {string} sha
 * @returns {{ files: number, injected: number }}
 */
export function injectBannerIntoDir(dir, sha) {
  const files = listHtmlFiles(dir);
  let injected = 0;
  for (const file of files) {
    const original = readFileSync(file, "utf8");
    const next = injectBannerIntoHtml(original, sha);
    if (next === original) {
      continue;
    }
    writeFileSync(file, next);
    injected += 1;
  }
  return { files: files.length, injected };
}

function main() {
  const [dir, sha] = process.argv.slice(2);
  if (!dir || !sha) {
    process.stderr.write("Usage: node scripts/showcase-inject-banner.mjs <dir> <sha>\n");
    process.exit(1);
  }
  if (!statSync(dir).isDirectory()) {
    process.stderr.write(`showcase-inject-banner: not a directory: ${dir}\n`);
    process.exit(1);
  }

  const result = injectBannerIntoDir(dir, sha);
  process.stdout.write(
    `showcase-inject-banner: ${result.injected} injected, ` +
      `${result.files - result.injected} unchanged (of ${result.files} html files) in ${dir}\n`,
  );
}

// Guard so importing this module (e.g. from tests) never runs main() as a
// side effect — mirrors the pure-function-extraction pattern in
// packages/create-zfb/bin/spawn-error-message.mjs.
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
