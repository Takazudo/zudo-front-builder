import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { auditBuiltAnchors, collectBuiltAnchors } from "../audit-wind-built-anchors.mjs";

function withDist(run) {
  const temp = mkdtempSync(join(tmpdir(), "wind-built-anchors-"));
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

test("captured baseline inventory has the expected wind and adjacent route counts", () => {
  withDist((dist) => {
    const result = auditBuiltAnchors(dist);
    assert.deepEqual(result.summary, {
      windRoutes: 116,
      windIds: 2082,
      windHeadingIds: 1618,
      adjacentRoutes: 42,
      adjacentIds: 682,
    });
    assert.equal(result.findings.filter((finding) => finding.type === "missing-route").length, 158);
  });
});

test("built anchor extraction distinguishes all ids from heading ids", () => {
  const anchors = collectBuiltAnchors(
    '<!doctype html><div id="section-anchor"></div><h2 id="heading-anchor">Heading</h2>',
  );

  assert.deepEqual(anchors.ids, ["section-anchor", "heading-anchor"]);
  assert.deepEqual(anchors.headingIds, ["heading-anchor"]);
});

test("the built audit requires every captured route, id, and heading id", () => {
  withDist((dist) => {
    const route = "docs/zudo-wind/utilities/gap/index.html";
    writeRoute(
      dist,
      route,
      '<!doctype html><main id="shell"><details><h2 id="old-topic">Old topic</h2></details></main>',
    );

    const result = auditBuiltAnchors(dist, {
      windBaseline: { [route]: { ids: ["shell", "old-topic"], headings: ["old-topic"] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.deepEqual(result.findings, []);
    assert.deepEqual(result.summary, {
      windRoutes: 1,
      windIds: 2,
      windHeadingIds: 1,
      adjacentRoutes: 0,
      adjacentIds: 0,
    });
  });
});

test("an old heading id on a non-heading element does not satisfy the built heading contract", () => {
  withDist((dist) => {
    const route = "docs/zudo-wind/utilities/gap/index.html";
    writeRoute(dist, route, '<!doctype html><div id="old-topic">Old topic</div>');

    const result = auditBuiltAnchors(dist, {
      windBaseline: { [route]: { ids: ["old-topic"], headings: ["old-topic"] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.deepEqual(result.findings, [
      { kind: "wind", route, type: "missing-heading-ids", ids: ["old-topic"] },
    ]);
  });
});

test("duplicate built ids fail the route audit", () => {
  withDist((dist) => {
    const route = "docs/zudo-wind/utilities/gap/index.html";
    writeRoute(
      dist,
      route,
      '<!doctype html><div id="shell"></div><h2 id="old-topic">Old</h2><p id="shell">Again</p>',
    );

    const result = auditBuiltAnchors(dist, {
      windBaseline: { [route]: { ids: ["shell", "old-topic"], headings: ["old-topic"] } },
      adjacentBaseline: noAdjacentRoutes,
    });
    assert.deepEqual(result.findings, [
      { kind: "wind", route, type: "duplicate-ids", ids: ["shell"] },
    ]);
  });
});

test("missing built routes are reported rather than inferred from source files", () => {
  withDist((dist) => {
    const route = "ja/docs/zudo-wind/utilities/padding/index.html";
    const result = auditBuiltAnchors(dist, {
      windBaseline: { [route]: { ids: ["old-topic"], headings: ["old-topic"] } },
      adjacentBaseline: noAdjacentRoutes,
    });

    assert.deepEqual(result.findings, [{ kind: "wind", route, type: "missing-route" }]);
  });
});
