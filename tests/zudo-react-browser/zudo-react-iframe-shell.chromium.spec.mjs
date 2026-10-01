// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

const pageName = "zudo-react-iframe-shell";

function documentGeneration(locator) {
  return locator.evaluate((element) => {
    if (!(element instanceof HTMLIFrameElement)) return null;
    return element.contentDocument?.body?.dataset.generation ?? null;
  });
}

test("iframe shells preserve parsed documents on hydrate and own navigation, mount, and cleanup", async ({
  page,
}) => {
  const state = await openHeldScenario(page, pageName);
  try {
    const hydrateFrame = page.locator("#frame-hydrate");
    const staticFrame = page.locator("#frame-static-src");
    await expect.poll(() => documentGeneration(hydrateFrame)).toBe("frame-hydrate-srcdoc-one");
    await expect.poll(() => documentGeneration(staticFrame)).toBe("src-one");

    const parsed = await page.evaluate(() => {
      const frame = document.querySelector("#frame-hydrate");
      const staticFrame = document.querySelector("#frame-static-src");
      if (!(frame instanceof HTMLIFrameElement) || !frame.contentDocument) {
        throw new Error("Parsed iframe shell or its same-origin srcdoc document is missing");
      }
      if (!(staticFrame instanceof HTMLIFrameElement) || !staticFrame.contentDocument) {
        throw new Error("Parsed static iframe or its same-origin document is missing");
      }
      window.__iframeBeforeHydrate = { frame, document: frame.contentDocument };
      window.__staticIframeBeforeHydrate = {
        frame: staticFrame,
        document: staticFrame.contentDocument,
      };
      const markerFrame = document.querySelector("#frame-markers");
      return {
        childCount: frame.childNodes.length,
        src: frame.getAttribute("src"),
        srcdoc: frame.getAttribute("srcdoc"),
        sandbox: frame.getAttribute("sandbox"),
        allow: frame.getAttribute("allow"),
        foreignMarker: frame.contentDocument.body.textContent,
        staticSrc: staticFrame.getAttribute("src"),
        staticSandbox: staticFrame.getAttribute("sandbox"),
        staticAllow: staticFrame.getAttribute("allow"),
        staticChildCount: staticFrame.childNodes.length,
        markerFallbackText: markerFrame?.textContent,
        markerFallbackChildCount: markerFrame?.childNodes.length,
        mountFrameCount: document.querySelectorAll("#frame-mount").length,
      };
    });
    expect(parsed).toMatchObject({
      childCount: 0,
      src: "/fixtures/zudo-react-iframe-shell/src-one.html",
      sandbox: "allow-same-origin",
      allow: "fullscreen",
      foreignMarker: expect.stringContaining("ZFB_FOREIGN_DOCUMENT_MARKER"),
      staticSrc: "/fixtures/zudo-react-iframe-shell/src-one.html",
      staticSandbox: "allow-same-origin",
      staticAllow: "fullscreen",
      staticChildCount: 0,
      markerFallbackText: expect.stringContaining("ZFB_IFRAME_FALLBACK_MARKER"),
      mountFrameCount: 0,
    });
    expect(parsed.srcdoc).toContain("frame-hydrate-srcdoc-one");
    expect(parsed.markerFallbackChildCount).toBeGreaterThan(0);

    await releaseScenario(page, state);
    const hydrated = await page.evaluate(() => {
      const frame = document.querySelector("#frame-hydrate");
      const staticFrame = document.querySelector("#frame-static-src");
      const entry = window.__zudoReactBrowser.roots[0];
      const markerEntry = window.__zudoReactBrowser.roots[2];
      const mountEntry = window.__zudoReactBrowser.roots[1];
      if (!(frame instanceof HTMLIFrameElement) || !(staticFrame instanceof HTMLIFrameElement)) {
        throw new Error("Hydrated iframe is missing");
      }
      window.__iframeAfterHydrate = frame.contentDocument;
      return {
        sameElement: window.__iframeBeforeHydrate.frame === frame,
        sameDocument: window.__iframeBeforeHydrate.document === frame.contentDocument,
        staticSameElement: window.__staticIframeBeforeHydrate.frame === staticFrame,
        staticSameDocument:
          window.__staticIframeBeforeHydrate.document === staticFrame.contentDocument,
        refMatches: entry.result.ref.current === frame,
        childCount: frame.childNodes.length,
        diagnostics: entry.diagnostics,
        markerDiagnostics: markerEntry.diagnostics,
        markerRoot: markerEntry.root,
        mountRoot: Boolean(mountEntry.root),
        mountDiagnostics: mountEntry.diagnostics,
      };
    });
    expect(hydrated).toEqual({
      sameElement: true,
      sameDocument: true,
      staticSameElement: true,
      staticSameDocument: true,
      refMatches: true,
      childCount: 0,
      diagnostics: [],
      markerDiagnostics: [
        expect.objectContaining({ code: "ZR_HYDRATION_MISMATCH", phase: "preflight" }),
      ],
      markerRoot: null,
      mountRoot: true,
      mountDiagnostics: [],
    });

    const mountFrame = page.locator("#frame-mount");
    await expect(mountFrame).toHaveCount(1);
    await expect.poll(() => documentGeneration(mountFrame)).toBe("frame-mount-srcdoc-one");
    const mounted = await page.evaluate(() => {
      const frame = document.querySelector("#frame-mount");
      const entry = window.__zudoReactBrowser.roots[1];
      if (!(frame instanceof HTMLIFrameElement)) throw new Error("Mounted iframe is missing");
      return {
        refMatches: entry.result.ref.current === frame,
        src: frame.getAttribute("src"),
        srcdoc: frame.getAttribute("srcdoc"),
        sandbox: frame.getAttribute("sandbox"),
        allow: frame.getAttribute("allow"),
        childCount: frame.childNodes.length,
      };
    });
    expect(mounted).toMatchObject({
      refMatches: true,
      src: "/fixtures/zudo-react-iframe-shell/src-one.html",
      sandbox: "allow-same-origin",
      allow: "fullscreen",
      childCount: 0,
    });
    expect(mounted.srcdoc).toContain("frame-mount-srcdoc-one");

    const secondSrcdoc =
      '<!doctype html><html><body data-generation="srcdoc-two"><p id="foreign-content">second foreign document</p></body></html>';
    const beforeSrcdocNavigation = await page.evaluate(
      () => window.__zudoReactBrowser.roots[0].result.loads.value,
    );
    await page.evaluate(async (html) => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.result.srcdoc.value = html;
      await window.__zudoReactBrowser.flush();
    }, secondSrcdoc);
    await expect.poll(() => documentGeneration(hydrateFrame)).toBe("srcdoc-two");
    await expect
      .poll(() => page.evaluate(() => window.__zudoReactBrowser.roots[0].result.loads.value))
      .toBeGreaterThan(beforeSrcdocNavigation);
    const srcdocNavigation = await page.evaluate(() => {
      const frame = document.querySelector("#frame-hydrate");
      if (!(frame instanceof HTMLIFrameElement)) throw new Error("Iframe disappeared");
      window.__iframeAfterSrcdocNavigation = frame.contentDocument;
      return {
        changedDocument: window.__iframeAfterHydrate !== frame.contentDocument,
        srcdoc: frame.getAttribute("srcdoc"),
      };
    });
    expect(srcdocNavigation.changedDocument).toBe(true);
    expect(srcdocNavigation.srcdoc).toBe(secondSrcdoc);

    await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.result.srcdoc.value = null;
      await window.__zudoReactBrowser.flush();
    });
    await expect.poll(() => documentGeneration(hydrateFrame)).toBe("src-one");
    const srcNavigation = await page.evaluate(() => {
      const frame = document.querySelector("#frame-hydrate");
      if (!(frame instanceof HTMLIFrameElement)) throw new Error("Iframe disappeared");
      window.__iframeAfterSrcNavigation = frame.contentDocument;
      return {
        changedDocument: frame.contentDocument !== window.__iframeAfterSrcdocNavigation,
        generation: frame.contentDocument?.body.dataset.generation,
      };
    });
    expect(srcNavigation).toEqual({ changedDocument: true, generation: "src-one" });

    await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      entry.result.src.value = "/fixtures/zudo-react-iframe-shell/src-two.html";
      await window.__zudoReactBrowser.flush();
    });
    await expect.poll(() => documentGeneration(hydrateFrame)).toBe("src-two");
    const secondSrcNavigation = await page.evaluate(() => {
      const element = document.querySelector("#frame-hydrate");
      if (!(element instanceof HTMLIFrameElement)) throw new Error("Iframe disappeared");
      return {
        changedDocument: element.contentDocument !== window.__iframeAfterSrcNavigation,
        src: element.getAttribute("src"),
        generation: element.contentDocument?.body.dataset.generation,
      };
    });
    expect(secondSrcNavigation).toEqual({
      changedDocument: true,
      src: "/fixtures/zudo-react-iframe-shell/src-two.html",
      generation: "src-two",
    });

    const removed = await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      const frame = entry.result.ref.current;
      if (!frame) throw new Error("Iframe ref was cleared before removal");
      const loadCount = entry.result.loads.value;
      entry.result.visible.value = false;
      await window.__zudoReactBrowser.flush();
      window.__iframeRemoved = frame;
      return {
        detached: !frame.isConnected,
        refCleared: entry.result.ref.current === null,
        loadCount,
      };
    });
    expect(removed.detached).toBe(true);
    expect(removed.refCleared).toBe(true);
    expect(removed.loadCount).toBeGreaterThan(0);
    await expect(hydrateFrame).toHaveCount(0);
    const afterRemoval = await page.evaluate(async () => {
      const entry = window.__zudoReactBrowser.roots[0];
      const frame = window.__iframeRemoved;
      const attrsBefore = { src: frame.getAttribute("src"), srcdoc: frame.getAttribute("srcdoc") };
      const loadsBefore = entry.result.loads.value;
      entry.result.src.value = "/fixtures/zudo-react-iframe-shell/src-one.html";
      entry.result.srcdoc.value = "<body data-generation='detached-update'>";
      await window.__zudoReactBrowser.flush();
      frame.dispatchEvent(new Event("load"));
      return {
        attrsBefore,
        attrsAfter: { src: frame.getAttribute("src"), srcdoc: frame.getAttribute("srcdoc") },
        loadsBefore,
        loadsAfter: entry.result.loads.value,
        refCleared: entry.result.ref.current === null,
      };
    });
    expect(afterRemoval).toEqual({
      attrsBefore: {
        src: "/fixtures/zudo-react-iframe-shell/src-two.html",
        srcdoc: null,
      },
      attrsAfter: {
        src: "/fixtures/zudo-react-iframe-shell/src-two.html",
        srcdoc: null,
      },
      loadsBefore: removed.loadCount,
      loadsAfter: removed.loadCount,
      refCleared: true,
    });

    const forbidden = await page.evaluate(async () => {
      const { islandRoot, renderToString } = await import("@takazudo/zfb/zudo-react/server");
      const h = window.__zudoReactBrowser.h;
      const rejected = (node) => {
        try {
          renderToString(
            islandRoot(node, { identity: { component: "IframeContract", build: "b1" } }),
          );
          return null;
        } catch (error) {
          return String(error);
        }
      };
      return {
        children: rejected(h("iframe", { children: "fallback" })),
        rawHtmlMarker: rejected(h("iframe", { rawHtml: "<!--zr:1:0:c-->fallback<!--/zr:1:0-->" })),
        noscript: rejected(h("noscript")),
        xmp: rejected(h("xmp")),
      };
    });
    expect(forbidden.children).toContain("ZR_PARSER_CONTEXT");
    expect(forbidden.rawHtmlMarker).toContain("ZR_RAW_HTML");
    expect(forbidden.noscript).toContain("ZR_PARSER_CONTEXT");
    expect(forbidden.xmp).toContain("ZR_PARSER_CONTEXT");
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
