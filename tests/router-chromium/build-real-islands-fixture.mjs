// Generates real server markup from the built zudo-react renderer for the router L4 test.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import * as react from "../../packages/zfb/dist/zudo-react/index.js";
import { islandRoot, renderToString } from "../../packages/zfb/dist/zudo-react/server.js";
import { createComponents } from "./fixture/real-islands-components.mjs";

const fixture = join(fileURLToPath(new URL(".", import.meta.url)), "fixture");
const components = createComponents(react);
const identity = (name) => ({ component: name, build: "browser-fixture-1" });
const island = (name, props, id, when = "load") =>
  renderToString(
    islandRoot(react.h(components[name], props), { identity: identity(name), when }),
  ).replace(/^<div /, `<div id="island-${id}" `);
const pages = {
  a: { second: { start: 10 }, idle: true, header: true },
  b: { second: { start: 20 }, idle: true, header: true },
  c: { second: { start: 20, component: "OtherCounter" }, idle: true, header: true },
  removed: { second: null, idle: true, header: true },
  gone: { header: false },
};
const nav = Object.keys(pages)
  .map((name) => `<a id="to-${name}" href="/real-islands-${name}.html">${name}</a>`)
  .join(" ");
for (const [name, config] of Object.entries(pages)) {
  const header = config.header
    ? `<header data-zfb-transition-persist="h">
    ${island("Toggle", {}, "toggle")}
    ${island("Counter", { id: "first", start: 1 }, "first")}
    ${config.second ? island(config.second.component ?? "Counter", { id: "second", start: config.second.start }, "second") : ""}
    ${config.idle ? island("Idle", { id: "deferred" }, "idle", "idle") : ""}
    <input id="persistent-input" value="abcdef" />
  </header>`
    : '<p id="no-header">header removed</p>';
  writeFileSync(
    join(fixture, `real-islands-${name}.html`),
    `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>real islands ${name}</title>
<meta name="zfb-view-transitions-enabled" content=""><meta name="zfb-view-transitions-fallback" content="animate">
<script type="importmap">{"imports":{"@takazudo/zfb/runtime":"/zfb-dist/runtime.js"}}</script>
</head><body><nav>${nav}</nav><h1>real islands ${name}</h1>${header}
<script type="module" src="/real-islands-bootstrap.js"></script></body></html>`,
  );
}
