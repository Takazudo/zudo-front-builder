import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { createComponents } from "./fixture/real-islands-components.mjs";

// The caller supplies either workspace-dist APIs or exact installed public exports.
export function buildNativeRegions({ Island, react, renderToString, fixture }) {
  const components = createComponents(react);
  globalThis.__zfb = {
    zudoReactBuild: "browser-fixture-1",
    zudoReactIslands: Object.keys(components),
  };
  const pages = { a: ["alpha", 10], b: ["alpha", 20], c: ["beta", 30], gone: [null, 40] };
  const nav = Object.keys(pages)
    .map((name) => `<a id="to-region-${name}" href="/real-islands-region-${name}.html">${name}</a>`)
    .join(" ");
  for (const [name, [context, start]] of Object.entries(pages)) {
    const counter = (id, persist) =>
      renderToString(
        Island({
          children: react.h(components.Counter, { id, start }),
          ...(persist ? { persist } : {}),
        }),
      );
    writeFileSync(
      join(fixture, `real-islands-region-${name}.html`),
      `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Regions ${name}</title>
<meta name="zfb-view-transitions-enabled" content=""><meta name="zfb-view-transitions-fallback" content="animate">
<link rel="icon" href="data:,">
<link rel="stylesheet" href="/native-regions.css">
<script type="importmap">{"imports":{"@takazudo/zfb/runtime":"/zfb-dist/runtime.js"}}</script>
</head><body data-page="${name}"><nav>${nav}</nav>
<header id="global-region">Fresh static region ${name}</header>
<div id="ancestor-region" data-zfb-transition-persist="ancestor">${counter("ancestor")}</div>
<div id="sdk-region">${counter("region-sdk", "region-sdk")}</div>
${context ? `<aside id="context-region" style="view-transition-name: context-${context}">Context ${context}: ${name}</aside>` : ""}
${name === "b" ? '<div id="optional-region">One-sided region</div>' : ""}
<main id="content-region">Animated content ${name}</main>
<script type="module" src="/real-islands-bootstrap.js"></script></body></html>`,
    );
  }
}
