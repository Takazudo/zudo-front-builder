import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { auditBuiltMarkdownLinks } from "../audit-wind-built-links.mjs";

function withDist(run) {
  const temp = mkdtempSync(join(tmpdir(), "wind-built-links-"));
  try {
    run(temp);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

function writeRoute(dist, route, html) {
  const filepath = join(dist, route);
  mkdirSync(dirname(filepath), { recursive: true });
  writeFileSync(filepath, html);
}

const noAdjacentRoutes = {};

test("the audit checks local Markdown links and fragments in captured wind pages", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const overview = "docs/zudo-wind/overview/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><h2 id="gap-start">Gap</h2><a href="/docs/zudo-wind/overview/#guide-start">Guide</a><a href="#gap-start">Top</a><a href="https://example.invalid/#external">External</a></body></html>',
    );
    writeRoute(
      dist,
      overview,
      '<!doctype html><html><body><h2 id="guide-start">Guide</h2></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: {
        [gap]: { ids: ["gap-start"] },
        [overview]: { ids: ["guide-start"] },
      },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.deepEqual(result.findings, []);
    assert.equal(result.summary.scopedRoutes, 2);
    assert.equal(result.summary.checkedScopedLinks, 2);
  });
});

test("a captured page fragment that has no emitted id fails even when its route exists", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="#renamed-heading">Old link</a></body></html>',
    );
    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.deepEqual(result.findings, [
      {
        source: gap,
        href: "#renamed-heading",
        type: "missing-fragment",
        fragment: "renamed-heading",
        target: gap,
      },
    ]);
  });
});

test("the audit checks inbound links across the built site under its configured prefix", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const overview = "docs/zudo-wind/overview/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><h2 id="legacy-topic">Legacy</h2></body></html>',
    );
    writeRoute(
      dist,
      overview,
      '<!doctype html><html><body><h2 id="overview">Overview</h2></body></html>',
    );
    writeRoute(
      dist,
      "docs/guides/inbound/index.html",
      '<!doctype html><html><body><a href="/wind-docs-preview/docs/zudo-wind/utilities/gap/#legacy-topic">Legacy wind fragment</a></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: ["legacy-topic"] }, [overview]: { ids: ["overview"] } },
      adjacentBaseline: noAdjacentRoutes,
      base: "/wind-docs-preview/",
    });
    assert.deepEqual(result.findings, []);
    assert.equal(result.summary.checkedInboundLinks, 1);
  });
});

test("root audits reject a wrong prefix even when its suffix exists", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const overview = "docs/zudo-wind/overview/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="/wrong-prefix/docs/zudo-wind/overview/#guide-start">Broken prefix</a></body></html>',
    );
    writeRoute(
      dist,
      overview,
      '<!doctype html><html><body><h2 id="guide-start">Guide</h2></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] }, [overview]: { ids: ["guide-start"] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].type, "missing-local-target");
    assert.equal(result.findings[0].href, "/wrong-prefix/docs/zudo-wind/overview/#guide-start");
  });
});

test("an emitted MDX link fails when only its built route exists", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const overview = "docs/zudo-wind/overview/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="/docs/zudo-wind/overview.mdx#guide-start">Source MDX path</a></body></html>',
    );
    writeRoute(
      dist,
      overview,
      '<!doctype html><html><body><h2 id="guide-start">Guide</h2></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] }, [overview]: { ids: ["guide-start"] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].type, "missing-local-target");
  });
});

test("dotted version route directories resolve to their emitted index page", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const release = "docs/changelog/zfb/v2.20.2/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="/docs/changelog/zfb/v2.20.2/#release-notes">Release</a></body></html>',
    );
    writeRoute(
      dist,
      release,
      '<!doctype html><html><body><h1 id="release-notes">Release notes</h1></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] } },
      adjacentBaseline: { [release]: { ids: ["release-notes"] } },
    });
    assert.deepEqual(result.findings, []);
    assert.equal(result.summary.checkedScopedLinks, 1);
  });
});

test("a configured prefix is required and same-origin canonical anchors are audited", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const overview = "docs/zudo-wind/overview/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="/docs/zudo-wind/overview/#guide-start">Unprefixed</a></body></html>',
    );
    writeRoute(
      dist,
      overview,
      '<!doctype html><html><body><h2 id="guide-start">Guide</h2></body></html>',
    );

    const mismatch = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] }, [overview]: { ids: ["guide-start"] } },
      adjacentBaseline: noAdjacentRoutes,
      base: "/wind-docs-preview/",
    });
    assert.equal(mismatch.findings[0].type, "wrong-base-prefix");

    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="https://zfb.takazudomodular.com/wind-docs-preview/docs/zudo-wind/overview/#guide-start">Canonical guide</a><a href="https://example.invalid/docs/zudo-wind/overview/">External</a></body></html>',
    );
    const prefixed = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] }, [overview]: { ids: ["guide-start"] } },
      adjacentBaseline: noAdjacentRoutes,
      base: "/wind-docs-preview/",
    });
    assert.deepEqual(prefixed.findings, []);
    assert.equal(prefixed.summary.checkedScopedLinks, 1);
  });
});

test("the inbound scan reports an exact unprefixed link to a captured route", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    const inbound = "docs/guides/inbound/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><h2 id="legacy-topic">Legacy</h2></body></html>',
    );
    writeRoute(
      dist,
      inbound,
      '<!doctype html><html><body><a href="/docs/zudo-wind/utilities/gap/#legacy-topic">Missing configured base</a></body></html>',
    );

    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: ["legacy-topic"] } },
      adjacentBaseline: noAdjacentRoutes,
      base: "/wind-docs-preview/",
    });
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].type, "wrong-base-prefix");
    assert.equal(result.findings[0].source, inbound);
    assert.equal(result.findings[0].target, "/docs/zudo-wind/utilities/gap/");
  });
});

test("the audit rejects malformed public bases", () => {
  withDist((dist) => {
    assert.throws(
      () =>
        auditBuiltMarkdownLinks(dist, { windBaseline: {}, adjacentBaseline: {}, base: "docs/" }),
      /Invalid docs URL base/,
    );
    assert.throws(
      () =>
        auditBuiltMarkdownLinks(dist, {
          windBaseline: {},
          adjacentBaseline: {},
          base: "/wind-docs-preview/../",
        }),
      /Invalid docs URL base/,
    );
  });
});

test("missing local destinations in captured pages fail instead of relying on strict-broken", () => {
  withDist((dist) => {
    const gap = "docs/zudo-wind/utilities/gap/index.html";
    writeRoute(
      dist,
      gap,
      '<!doctype html><html><body><a href="../missing/#gone">Missing</a></body></html>',
    );
    const result = auditBuiltMarkdownLinks(dist, {
      windBaseline: { [gap]: { ids: [] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].type, "missing-local-target");
  });
});
