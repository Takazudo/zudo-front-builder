// Tests for scripts/showcase-inject-banner.mjs (issue #2282).
//
// Strategy: exercise the exported pure functions directly for the
// string-transform behavior (idempotency, no-<body> skip, content
// preservation), then drive injectBannerIntoDir() over a real tmpdir tree
// to prove the recursive *.html discovery + write-only-when-changed
// behavior end to end. Mirrors the tmpdir-fixture style used in
// packages/create-zfb/bin/__tests__/spawn-error-message.test.mjs.

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  BANNER_MARKER,
  buildBannerHtml,
  injectBannerIntoDir,
  injectBannerIntoHtml,
} from "../showcase-inject-banner.mjs";

const SHA = "abc1234def5678901234567890123456789abcd";

function countOccurrences(haystack, needle) {
  return haystack.split(needle).length - 1;
}

describe("buildBannerHtml", () => {
  it("names the commit and links to the docs site and the GitHub repo", () => {
    const banner = buildBannerHtml(SHA);
    expect(banner).toContain("npm create zfb@latest my-site");
    expect(banner).toContain(SHA.slice(0, 7));
    expect(banner).toContain("https://zfb.takazudomodular.com");
    expect(banner).toContain("https://github.com/Takazudo/zudo-front-builder");
    expect(banner).toContain(`https://github.com/Takazudo/zudo-front-builder/commit/${SHA}`);
  });

  it("uses only inline styles — no <style> tag, no class attribute", () => {
    const banner = buildBannerHtml(SHA);
    expect(banner).not.toContain("<style");
    expect(banner).not.toContain("class=");
  });
});

describe("injectBannerIntoHtml", () => {
  it("injects immediately after the <body ...> open tag", () => {
    const html = '<html><head></head><body class="foo"><h1>Hi</h1></body></html>';
    const result = injectBannerIntoHtml(html, SHA);
    const bodyOpenEnd = result.indexOf('<body class="foo">') + '<body class="foo">'.length;
    expect(result.slice(bodyOpenEnd, bodyOpenEnd + BANNER_MARKER.length)).toBe(BANNER_MARKER);
    expect(result).toContain("<h1>Hi</h1>");
  });

  it("is idempotent: injecting twice produces exactly one banner", () => {
    const html = "<html><body><main>content</main></body></html>";
    const once = injectBannerIntoHtml(html, SHA);
    const twice = injectBannerIntoHtml(once, SHA);

    expect(twice).toBe(once);
    expect(countOccurrences(twice, "npm create zfb@latest my-site")).toBe(1);
    // BANNER_MARKER wraps the block (open + close), so exactly one banner
    // means exactly two occurrences of the marker string.
    expect(countOccurrences(twice, BANNER_MARKER)).toBe(2);
  });

  it("preserves the original body content byte-for-byte apart from the injected block", () => {
    const html =
      "<!doctype html><html><head><title>t</title></head>" +
      '<body id="page" data-x="1"><header>Header</header><main>Body copy stays exact.</main>' +
      "<footer>&copy; 2026</footer></body></html>";

    const result = injectBannerIntoHtml(html, SHA);
    const banner = buildBannerHtml(SHA);

    expect(result).toBe(html.replace(/<body\b[^>]*>/i, (tag) => tag + banner));
    // Stripping exactly the injected block back out must reconstruct the
    // original document exactly.
    expect(result.replace(banner, "")).toBe(html);
  });

  it("leaves an HTML file with no <body> completely unchanged", () => {
    const html = "<html><head><title>fragment, no body tag</title></head></html>";
    const result = injectBannerIntoHtml(html, SHA);
    expect(result).toBe(html);
    expect(result).not.toContain(BANNER_MARKER);
  });
});

describe("injectBannerIntoDir", () => {
  let dir;

  afterEach(() => {
    if (dir) {
      rmSync(dir, { recursive: true, force: true });
      dir = undefined;
    }
  });

  it("injects into every *.html file recursively, skips non-html files, and is idempotent on re-run", () => {
    dir = mkdtempSync(join(tmpdir(), "showcase-inject-banner-test-"));
    mkdirSync(join(dir, "blog"), { recursive: true });

    const indexHtml = "<html><body><h1>Home</h1></body></html>";
    const postHtml = "<html><body><h1>Post</h1></body></html>";
    const noBodyHtml = "<html><head><title>no body</title></head></html>";
    const notHtml = "body { color: red; }";

    writeFileSync(join(dir, "index.html"), indexHtml);
    writeFileSync(join(dir, "blog", "post.html"), postHtml);
    writeFileSync(join(dir, "blog", "fragment.html"), noBodyHtml);
    writeFileSync(join(dir, "styles.css"), notHtml);

    const first = injectBannerIntoDir(dir, SHA);
    expect(first.files).toBe(3);
    expect(first.injected).toBe(2); // index.html + blog/post.html; fragment.html has no <body>

    expect(readFileSync(join(dir, "index.html"), "utf8")).toContain(BANNER_MARKER);
    expect(readFileSync(join(dir, "blog", "post.html"), "utf8")).toContain(BANNER_MARKER);
    // No-<body> file left byte-unchanged.
    expect(readFileSync(join(dir, "blog", "fragment.html"), "utf8")).toBe(noBodyHtml);
    // Non-html file untouched.
    expect(readFileSync(join(dir, "styles.css"), "utf8")).toBe(notHtml);

    // Re-running over the now-injected tree must not add a second banner
    // anywhere, and must report nothing newly injected.
    const second = injectBannerIntoDir(dir, SHA);
    expect(second.files).toBe(3);
    expect(second.injected).toBe(0);

    const indexAfterTwice = readFileSync(join(dir, "index.html"), "utf8");
    expect(countOccurrences(indexAfterTwice, BANNER_MARKER)).toBe(2);
  });
});
