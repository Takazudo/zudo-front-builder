import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  EXPECTED_ISLANDS,
  checkIslandMarkers,
  collectIslandMarkers,
  hasIslandManifestEntry,
} from "../assert-island-markers.mjs";

function fixtureDist({ markerOverrides = {}, omit = [] } = {}) {
  const dist = mkdtempSync(join(tmpdir(), "zfb-island-guard-"));
  const assets = join(dist, "assets");
  mkdirSync(assets, { recursive: true });
  const registrations = EXPECTED_ISLANDS.map(
    ({ marker }) =>
      `__zfb_register(__zfb_island, "default", "${marker}", "/components/${marker}.tsx");`,
  ).join("\n");
  writeFileSync(join(assets, "islands-fixture.js"), registrations);

  for (const locale of ["", "ja/"]) {
    for (const { route, marker } of EXPECTED_ISLANDS) {
      const relativeRoute = `${locale}${route}`;
      const markerValue = markerOverrides[relativeRoute] ?? marker;
      const pagePath = join(dist, relativeRoute, "index.html");
      mkdirSync(join(dist, relativeRoute), { recursive: true });
      const markerHtml = omit.includes(relativeRoute)
        ? "<main>playground omitted</main>"
        : `<main><div data-zfb-island=${markerValue}></div></main>`;
      writeFileSync(pagePath, `<!doctype html>${markerHtml}`);
    }
  }
  return dist;
}

test("collectIslandMarkers accepts quoted, unquoted, and SSR-skip attributes", () => {
  assert.deepEqual(
    [
      ...collectIslandMarkers(
        `<div data-zfb-island="A"></div><div data-zfb-island-skip-ssr='B'></div><div data-zfb-island=C></div>`,
      ),
    ].sort(),
    ["A", "B", "C"],
  );
});

test("collectIslandMarkers ignores marker-looking prose outside element attributes", () => {
  assert.deepEqual([...collectIslandMarkers(`<p>data-zfb-island="NotAnElementAttribute"</p>`)], []);
});

test("manifest check requires a registry-key position, not a stray marker string", () => {
  assert.equal(
    hasIslandManifestEntry(
      '__zfb_register(ns, "default", "RenderPlayground", "src");',
      "RenderPlayground",
    ),
    true,
  );
  assert.equal(
    hasIslandManifestEntry('const source = "RenderPlayground";', "RenderPlayground"),
    false,
  );
});

test("positive fixture passes all eight route/marker pairs and both halves", () => {
  const dist = fixtureDist();
  try {
    assert.deepEqual(checkIslandMarkers(dist).findings, []);
  } finally {
    rmSync(dist, { recursive: true, force: true });
  }
});

test("removing an MDX island marker fails the marker half", () => {
  const dist = fixtureDist({ omit: ["docs/playground/parse"] });
  try {
    const { findings } = checkIslandMarkers(dist);
    const parseMarkerFailures = findings.filter(
      ({ route, marker, half }) =>
        route === "/docs/playground/parse" && marker === "ParsePlayground" && half === "marker",
    );
    assert.equal(parseMarkerFailures.length, 1);
    assert.match(parseMarkerFailures[0].message, /marker half failed/);
  } finally {
    rmSync(dist, { recursive: true, force: true });
  }
});

test("renaming a displayName fails the manifest half for the new emitted marker", () => {
  const dist = fixtureDist({
    markerOverrides: { "docs/playground/render": "RenamedRenderPlayground" },
  });
  try {
    const { findings } = checkIslandMarkers(dist);
    const renamedManifestFailures = findings.filter(
      ({ route, marker, half }) =>
        route === "/docs/playground/render" &&
        marker === "RenamedRenderPlayground" &&
        half === "manifest",
    );
    assert.equal(renamedManifestFailures.length, 1);
    assert.match(renamedManifestFailures[0].message, /manifest half failed/);
  } finally {
    rmSync(dist, { recursive: true, force: true });
  }
});
