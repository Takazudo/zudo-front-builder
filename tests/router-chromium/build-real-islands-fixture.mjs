// Generates real server markup from the built zudo-react renderer for the router L4 test.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Island } from "../../packages/zfb/dist/index.js";
import * as react from "../../packages/zfb/dist/zudo-react/index.js";
import { jsx } from "../../packages/zfb/dist/zudo-react/jsx-runtime.js";
import { islandRoot, renderToString } from "../../packages/zfb/dist/zudo-react/server.js";
import { createComponents } from "./fixture/real-islands-components.mjs";

const fixture = join(fileURLToPath(new URL(".", import.meta.url)), "fixture");
const components = createComponents(react);
globalThis.__zfb = {
  zudoReactBuild: "browser-fixture-1",
  zudoReactIslands: Object.keys(components),
};
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

const sdkPages = {
  a: { same: 2, refresh: 10, retain: 100, present: true },
  b: { same: 2, refresh: 20, retain: 200, present: true },
  gone: { present: false },
};
const sdkNav = Object.keys(sdkPages)
  .map((name) => `<a id="to-sdk-${name}" href="/real-islands-sdk-${name}.html">${name}</a>`)
  .join(" ");
const sdkIsland = (key, id, start, persistProps = false) =>
  renderToString(
    Island({
      children: jsx(components.Counter, { id, start }),
      persist: `sdk-${key}`,
      persistProps,
    }),
  ).replace(/^<div /, `<div id="sdk-island-${key}" `);

for (const [name, config] of Object.entries(sdkPages)) {
  const persistedIslands = config.present
    ? [
        sdkIsland("same", "sdk-same", config.same),
        sdkIsland("refresh", "sdk-refresh", config.refresh),
        sdkIsland("retain", "sdk-retain", config.retain, true),
      ].join("\n")
    : '<p id="sdk-no-targets">persist targets removed</p>';
  writeFileSync(
    join(fixture, `real-islands-sdk-${name}.html`),
    `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>SDK persistence ${name}</title>
<meta name="zfb-view-transitions-enabled" content=""><meta name="zfb-view-transitions-fallback" content="animate">
<script type="importmap">{"imports":{"@takazudo/zfb/runtime":"/zfb-dist/runtime.js"}}</script>
</head><body><nav>${sdkNav}</nav><h1>SDK persistence ${name}</h1><main>${persistedIslands}</main>
<script type="module" src="/real-islands-bootstrap.js"></script></body></html>`,
  );
}

// Pending visible islands inside a retained ancestor. The spacer keeps the
// native IntersectionObserver from firing until the browser actually scrolls.
for (const [layout, labels] of Object.entries({
  skip: { a: "Skip A", b: "Skip B" },
  text: { a: "Text A", b: "Text B", c: "Text B" },
})) {
  for (const [name, label] of Object.entries(labels)) {
    const nav = Object.keys(labels)
      .map(
        (target) =>
          `<a id="to-pending-${target}" href="/real-islands-pending-${layout}-${target}.html">${target}</a>`,
      )
      .join(" ");
    const pending = renderToString(
      Island({
        when: "visible",
        ...(layout === "skip" ? { ssrFallback: jsx("div", { children: "Await visibility" }) } : {}),
        children: jsx(components.Counter, { id: "pending", start: 0, label }),
      }),
    ).replace(/^<div /, '<div id="island-pending" ');
    writeFileSync(
      join(fixture, `real-islands-pending-${layout}-${name}.html`),
      `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>pending ${layout} ${name}</title>
<meta name="zfb-view-transitions-enabled" content=""><meta name="zfb-view-transitions-fallback" content="animate">
<script type="importmap">{"imports":{"@takazudo/zfb/runtime":"/zfb-dist/runtime.js"}}</script>
</head><body><nav>${nav}</nav><h1>pending ${layout} ${name}</h1>
<header data-zfb-transition-persist="pending-header"><div style="height: 2000px"></div>${pending}</header>
<script type="module" src="/real-islands-bootstrap.js"></script></body></html>`,
    );
  }
}
