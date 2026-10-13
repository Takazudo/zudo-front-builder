// Active native snapshots, not live DOM geometry, are the visual contract (#4205).
import { writeFile } from "node:fs/promises";
import { test, expect } from "@playwright/test";

async function attachJson(testInfo, name, value) {
  const path = testInfo.outputPath(name);
  await writeFile(path, JSON.stringify(value, null, 2));
  await testInfo.attach(name, { path, contentType: "application/json" });
}

const path = (name) => `/real-islands-region-${name}.html`;
const protectedNames = ["global-region", "ancestor-region", "sdk-region"];
const clips = {
  "global-region": { x: 20, y: 50, width: 560, height: 80 },
  "ancestor-region": { x: 20, y: 130, width: 560, height: 80 },
  "sdk-region": { x: 20, y: 210, width: 560, height: 80 },
  "context-alpha": { x: 20, y: 290, width: 560, height: 80 },
  "optional-region": { x: 20, y: 370, width: 560, height: 80 },
  "content-region": { x: 0, y: 450, width: 600, height: 80 },
};

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (error) => {
    throw error;
  });
  await page.addInitScript(() => {
    window.__regionProof = { records: [], loads: 0, hold: true, errors: [] };
    addEventListener("unhandledrejection", (event) =>
      window.__regionProof.errors.push(String(event.reason)),
    );
    document.addEventListener("zfb:page-load", () => window.__regionProof.loads++);
    document.addEventListener("zfb:before-swap", (event) => {
      const proof = window.__regionProof;
      const vt = event.viewTransition;
      const record = {
        to: event.to.pathname,
        ready: "pending",
        update: "pending",
        finished: "pending",
      };
      proof.records.push(record);
      proof.active = vt;
      // Register and return: ready cannot settle until this swap callback returns.
      for (const [key, promise] of [
        ["update", vt.updateCallbackDone],
        ["finished", vt.finished],
      ]) {
        promise.then(
          () => (record[key] = "fulfilled"),
          (error) => (record[key] = error.name),
        );
      }
      vt.ready.then(
        () => {
          record.ready = "fulfilled";
          proof.animations = document.getAnimations();
          record.animations = proof.animations.map((a) => ({
            name: a.animationName,
            pseudo: a.effect?.pseudoElement,
          }));
          if (proof.hold) {
            // Seek real native animations to two active frames; never finish/cancel them for a screenshot.
            for (const animation of proof.animations) {
              animation.pause();
              animation.currentTime = 240;
            }
          }
        },
        (error) => (record.ready = error.name),
      );
      if (proof.skip) vt.skipTransition();
      if (proof.unnamed)
        event.newDocument.querySelector("#global-region").style.viewTransitionName = "none";
    });
  });
});

async function open(page) {
  await page.goto(path("a"));
  await page.waitForFunction(
    () => window.__regionProof.loads === 1 && window.__realIslands?.mounts.length === 2,
  );
}

async function navigate(page, target) {
  const count = await page.evaluate(() => window.__regionProof.records.length);
  if (target === "back" || target === "forward")
    await page.evaluate((direction) => history[direction](), target);
  else await page.locator(`#to-region-${target}`).evaluate((link) => link.click());
  await page.waitForFunction((previous) => {
    const proof = window.__regionProof;
    return (
      proof.records.length > previous &&
      proof.records.at(-1).ready !== "pending" &&
      proof.records.at(-1).update !== "pending"
    );
  }, count);
}

async function snapshots(page) {
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("DOM.enable");
    await cdp.send("CSS.enable");
    const { root } = await cdp.send("DOM.getDocument", { depth: -1, pierce: true });
    const nodes = [];
    function visit(node, parent) {
      if (node.pseudoType?.startsWith("view-transition"))
        nodes.push({
          id: node.nodeId,
          type: node.pseudoType,
          name: node.pseudoIdentifier,
          parent: parent?.pseudoType,
        });
      for (const child of [...(node.children ?? []), ...(node.pseudoElements ?? [])])
        visit(child, node);
    }
    visit(root);
    // CDP proves these pseudo-elements actually exist; getComputedStyle alone cannot.
    for (const node of nodes) {
      const { computedStyle } = await cdp.send("CSS.getComputedStyleForNode", { nodeId: node.id });
      node.style = Object.fromEntries(
        computedStyle
          .filter(({ name }) =>
            ["animation-name", "opacity", "transform", "mix-blend-mode"].includes(name),
          )
          .map(({ name, value }) => [name, value]),
      );
    }
    return nodes;
  } finally {
    await cdp.detach();
  }
}

function stationary(nodes, names) {
  return names.every((name) => {
    const group = nodes.find((node) => node.name === name && node.type === "view-transition-group");
    const images = nodes.filter(
      (node) =>
        node.name === name && ["view-transition-old", "view-transition-new"].includes(node.type),
    );
    return (
      group?.parent === "view-transition" &&
      images.length > 0 &&
      [group, ...images].every(
        (node) => node.style["animation-name"] === "none" && node.style.opacity === "1",
      )
    );
  });
}

async function activeProof(page, testInfo, label, names, expectedStationary = true) {
  const nodes = await snapshots(page);
  expect(
    nodes.some((node) => node.name === "content-region" && node.type === "view-transition-new"),
  ).toBe(true);
  const record = await page.evaluate(() => window.__regionProof.records.at(-1));
  expect(record.ready).toBe("fulfilled");
  expect(record.update).toBe("fulfilled");
  expect(record.finished).toBe("pending");
  // Fail closed if this browser does not expose pseudo-element identifiers.
  expect(
    record.animations.some(
      (a) => a.pseudo === "::view-transition-new(content-region)" && a.name === "region-in",
    ),
  ).toBe(true);
  const frames = [];
  for (const time of [240, 840]) {
    await page.evaluate(async (time) => {
      for (const animation of window.__regionProof.animations) animation.currentTime = time;
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }, time);
    const frame = {};
    for (const name of [...names, "content-region"]) {
      const imagePath = testInfo.outputPath(`${label}-${name}-${time}.png`);
      frame[name] = await page.screenshot({
        path: imagePath,
        clip: clips[name],
        animations: "allow",
      });
      await testInfo.attach(`${label}-${name}-${time}.png`, {
        path: imagePath,
        contentType: "image/png",
      });
    }
    frames.push(frame);
  }
  const pixelsStationary = names.every((name) => frames[0][name].equals(frames[1][name]));
  await attachJson(testInfo, `${label}-native.json`, {
    browser: await page.context().browser().version(),
    record,
    nodes,
    pixelsStationary,
  });
  expect(
    frames[0]["content-region"].equals(frames[1]["content-region"]),
    "positive animated-content control",
  ).toBe(false);
  expect(stationary(nodes, names), "native group + old/new animation policy").toBe(
    expectedStationary,
  );
  expect(pixelsStationary, "same active-frame pixel oracle for recipe and mutation").toBe(
    expectedStationary,
  );

  expect(await page.evaluate(() => window.__regionProof.records.at(-1).finished)).toBe("pending");
  return nodes;
}

async function finish(page) {
  await page.evaluate(() => {
    for (const animation of window.__regionProof.animations) animation.finish();
  });
  await page.waitForFunction(() => window.__regionProof.records.at(-1).finished === "fulfilled");
  expect(await page.evaluate(() => window.__regionProof.errors)).toEqual([]);
}

test("stationary snapshots survive static replacement and fresh island remounts", async ({
  page,
}, testInfo) => {
  await open(page);
  await page.evaluate(
    () =>
      (window.__regionNodes = [
        document.querySelector("#global-region"),
        document.querySelector("#ancestor-region"),
        document.querySelector("#sdk-region > div"),
      ]),
  );
  await navigate(page, "b");
  const nodes = await activeProof(page, testInfo, "same-context", [
    ...protectedNames,
    "context-alpha",
    "optional-region",
  ]);
  expect(
    nodes
      .filter((n) => n.name === "optional-region" && /view-transition-(old|new)$/.test(n.type))
      .map((n) => n.type),
  ).toEqual(["view-transition-new"]);
  await expect(page.locator("#global-region")).toHaveText("Fresh static region b");
  await expect(page.locator("#value-ancestor")).toHaveText("20");
  await expect(page.locator("#value-region-sdk")).toHaveText("20");
  expect(
    await page.evaluate(() =>
      [
        document.querySelector("#global-region"),
        document.querySelector("#ancestor-region"),
        document.querySelector("#sdk-region > div"),
      ].map((node, i) => node === window.__regionNodes[i]),
    ),
  ).toEqual([false, true, true]);
  expect(
    await page.evaluate(() => window.__realIslands.disposals.map(({ id }) => id).sort()),
  ).toEqual(["ancestor", "region-sdk"]);
  expect(
    await page.evaluate(() =>
      window.__realIslands.mounts.slice(2).map(({ id, mode }) => [id, mode]),
    ),
  ).toEqual([
    ["ancestor", "render"],
    ["region-sdk", "render"],
  ]);
  await finish(page);
  await page.locator("#button-region-sdk").click();
  await expect(page.locator("#value-region-sdk")).toHaveText("21");
});

test("changed context, one-sided exit and Back/Forward retain the independent policy", async ({
  page,
}, testInfo) => {
  await open(page);
  await navigate(page, "b");
  await finish(page);
  for (const target of ["c", "back", "forward"]) {
    await navigate(page, target);
    const nodes = await activeProof(page, testInfo, target, [...protectedNames, "optional-region"]);
    for (const name of ["context-alpha", "context-beta"]) {
      const images = nodes.filter(
        (n) => n.name === name && /view-transition-(old|new)$/.test(n.type),
      );
      expect(images).toHaveLength(1);
      const animations = await page.evaluate(() => window.__regionProof.records.at(-1).animations);
      expect(
        animations.some(
          (a) =>
            a.pseudo === `::${images[0].type}(${name})` &&
            ["region-in", "region-out"].includes(a.name),
        ),
      ).toBe(true);
    }
    const optional = nodes.filter(
      (n) => n.name === "optional-region" && /view-transition-(old|new)$/.test(n.type),
    );
    expect(optional.map((n) => n.type)).toEqual([
      target === "back" ? "view-transition-new" : "view-transition-old",
    ]);
    await finish(page);
    await expect(page.locator("#value-region-sdk")).toHaveText(target === "back" ? "20" : "30");
  }
});

for (const layer of ["old", "new", "group"]) {
  test(`negative control: removing the ${layer} protection fails the same oracle`, async ({
    page,
  }, testInfo) => {
    await open(page);
    await page.evaluate((layer) => {
      {
        // Deliberate test-only motion makes each omitted layer visibly discriminating,
        // even when a stable group or an opaque incoming image hides the UA fade.
        const style = document.createElement("style");
        style.textContent = `@keyframes probe-motion { from { translate: 0px; z-index: 1; } to { translate: 100px; z-index: 1; } } ::view-transition-${layer}(global-region) { animation: probe-motion 1200ms linear both; }`;
        style.setAttribute("data-zfb-transition-persist", "probe-motion");
        document.head.insertBefore(style, document.querySelector('link[rel="stylesheet"]'));
        document.addEventListener("zfb:before-swap", (event) =>
          event.newDocument.head.append(style.cloneNode(true)),
        );
      }
      const sheet = [...document.styleSheets].find((s) => s.href?.endsWith("native-regions.css"));
      const selector = `::view-transition-${layer}(global-region)`;
      for (const rule of sheet.cssRules) {
        if (
          rule.selectorText
            ?.split(",")
            .map((s) => s.trim())
            .includes(selector) &&
          rule.style.animationName === "none"
        ) {
          rule.selectorText = rule.selectorText
            .split(",")
            .filter((s) => s.trim() !== selector)
            .join(",");
        }
      }
    }, layer);
    await navigate(page, "b");
    await activeProof(page, testInfo, `missing-${layer}`, ["global-region"], false);
    await finish(page);
  });
}

test("view-transition-name none leaves the region inside the animated root snapshot", async ({
  page,
}, testInfo) => {
  await open(page);
  await page.evaluate(() => {
    window.__regionProof.unnamed = true;
    document.querySelector("#global-region").style.viewTransitionName = "none";
  });
  await navigate(page, "b");
  await activeProof(page, testInfo, "unnamed-in-root", ["global-region"], false);
  await finish(page);
});

test("superseding and skipped visuals still complete their DOM updates", async ({
  page,
}, testInfo) => {
  await open(page);
  await navigate(page, "b");
  await navigate(page, "c");
  await activeProof(page, testInfo, "superseding", protectedNames);
  await finish(page);
  await page.evaluate(() => {
    window.__regionProof.skip = true;
  });
  await navigate(page, "a");
  await page.waitForFunction(() => window.__regionProof.records.at(-1).finished === "fulfilled");
  const records = await page.evaluate(() => window.__regionProof.records);
  expect(records.map((r) => r.update)).toEqual(["fulfilled", "fulfilled", "fulfilled"]);
  expect(records.map((r) => r.finished)).toEqual(["fulfilled", "fulfilled", "fulfilled"]);
  expect(records.at(-1).ready).toBe("AbortError");
  await expect(page.locator("#value-region-sdk")).toHaveText("10");
  expect(await page.evaluate(() => window.__regionProof.errors)).toEqual([]);
  await attachJson(testInfo, "promise-outcomes.json", records);
});

for (const mode of ["reduced-motion", "no-native"]) {
  test(`${mode} retains fresh destination content without requiring snapshot animations`, async ({
    page,
  }, testInfo) => {
    if (mode === "reduced-motion") await page.emulateMedia({ reducedMotion: "reduce" });
    else
      await page.addInitScript(() => {
        document.startViewTransition = undefined;
      });
    await open(page);
    await page.evaluate(() => (window.__regionProof.hold = false));
    await navigate(page, "b");
    await page.waitForFunction(() => window.__regionProof.records.at(-1).finished === "fulfilled");
    const record = await page.evaluate(() => window.__regionProof.records.at(-1));
    expect([record.ready, record.update, record.finished]).toEqual([
      "fulfilled",
      "fulfilled",
      "fulfilled",
    ]);
    expect(record.animations).toEqual([]);
    await expect(page.locator("#value-ancestor")).toHaveText("20");
    await expect(page.locator("#value-region-sdk")).toHaveText("20");
    expect(await page.evaluate(() => window.__regionProof.errors)).toEqual([]);
    await attachJson(testInfo, `${mode}.json`, record);
  });
}

test("native animation advances on the browser clock without seeking", async ({
  page,
}, testInfo) => {
  await open(page);
  await page.evaluate(() => (window.__regionProof.hold = false));
  await navigate(page, "b");
  const sample = await page.evaluate(async () => {
    const proof = window.__regionProof;
    const animation = proof.animations.find(
      (a) => a.effect?.pseudoElement === "::view-transition-new(content-region)",
    );
    if (!animation || animation.playState !== "running")
      throw new Error("No running native content animation");
    const first = animation.currentTime;
    await new Promise((resolve) => {
      function frame() {
        if (animation.currentTime > first + 120) resolve();
        else requestAnimationFrame(frame);
      }
      requestAnimationFrame(frame);
    });
    return {
      first,
      second: animation.currentTime,
      playState: animation.playState,
      finished: proof.records.at(-1).finished,
    };
  });
  expect(sample.second).toBeGreaterThan(sample.first + 120);
  expect(sample.playState).toBe("running");
  expect(sample.finished).toBe("pending");
  await attachJson(testInfo, "browser-clock.json", sample);
  // The bounded pixel/geometry proofs above seek paused *native* snapshots;
  // this separate control proves those animations also advance without seeking.
  await page.waitForFunction(() => window.__regionProof.records.at(-1).finished === "fulfilled");
});
