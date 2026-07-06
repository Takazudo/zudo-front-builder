/**
 * @vitest-environment happy-dom
 */
// Unit tests for client-router/swap-functions.
//
// The functions under test are pure DOM helpers — happy-dom gives us a real
// `document` global so we can assert the post-swap shape directly. We exercise
// the public surface (`deselectScripts`, `swapHeadElements`, `swapBodyElement`,
// `swapRootAttributes`, `saveFocus`/`restoreFocus`, `detectScriptExecuted`)
// because that's the contract `router.ts` depends on; covering each independently
// keeps a regression localised when one part shifts.

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { drainHappyDom, htmlDoc, installHappyDomShim, resetDocument } from "./_helpers.js";

installHappyDomShim();

import {
  deselectScripts,
  detectScriptExecuted,
  restoreFocus,
  saveFocus,
  swap,
  swapBodyElement,
  swapHeadElements,
  swapRootAttributes,
} from "../../client-router/swap-functions.js";

const PERSIST_ATTR = "data-zfb-transition-persist";

beforeEach(resetDocument);
afterEach(drainHappyDom);

describe("swapHeadElements — head dedup", () => {
  it("keeps an existing stylesheet when the new head has the same href", () => {
    document.head.innerHTML = `<link rel="stylesheet" href="/site.css">`;
    const oldLink = document.head.querySelector('link[href="/site.css"]')!;
    const newDoc = htmlDoc(`<!doctype html><html><head>
      <link rel="stylesheet" href="/site.css">
    </head><body></body></html>`);

    swapHeadElements(newDoc);

    const links = document.head.querySelectorAll('link[rel="stylesheet"][href="/site.css"]');
    expect(links).toHaveLength(1);
    // The original link must remain — confirms dedup, not just count.
    expect(links[0]).toBe(oldLink);
  });

  it("preserves a persist-id'd element across swaps when the new head includes the same id", () => {
    document.head.innerHTML = `<meta ${PERSIST_ATTR}="theme" name="color-scheme" content="dark">`;
    const oldMeta = document.head.querySelector("meta[name=color-scheme]")!;
    const newDoc = htmlDoc(`<!doctype html><html><head>
      <meta ${PERSIST_ATTR}="theme" name="color-scheme" content="dark">
    </head><body></body></html>`);

    swapHeadElements(newDoc);

    const metas = document.head.querySelectorAll("meta[name=color-scheme]");
    expect(metas).toHaveLength(1);
    expect(metas[0]).toBe(oldMeta);
  });

  it("removes head elements that the new document does not contain", () => {
    document.head.innerHTML = `
      <link rel="stylesheet" href="/old.css">
      <meta name="description" content="old">
    `;
    const newDoc = htmlDoc(
      `<!doctype html><html><head><meta name="description" content="new"></head><body></body></html>`,
    );

    swapHeadElements(newDoc);

    expect(document.head.querySelector('link[href="/old.css"]')).toBeNull();
    const desc = document.head.querySelector("meta[name=description]") as HTMLMetaElement;
    expect(desc).not.toBeNull();
    expect(desc.content).toBe("new");
  });

  it("appends head elements that exist only in the new document", () => {
    document.head.innerHTML = "";
    const newDoc = htmlDoc(`<!doctype html><html><head>
      <link rel="stylesheet" href="/added.css">
      <meta name="theme-color" content="#222">
    </head><body></body></html>`);

    swapHeadElements(newDoc);

    expect(document.head.querySelector('link[href="/added.css"]')).not.toBeNull();
    expect(document.head.querySelector('meta[name="theme-color"]')).not.toBeNull();
  });

  it("preserves an inline <style> with identical text content (FOUT prevention)", () => {
    const css = ".x { color: red; }";
    document.head.innerHTML = `<style>${css}</style>`;
    const oldStyle = document.head.querySelector("style")!;
    const newDoc = htmlDoc(
      `<!doctype html><html><head><style>${css}</style></head><body></body></html>`,
    );

    swapHeadElements(newDoc);

    const styles = document.head.querySelectorAll("style");
    expect(styles).toHaveLength(1);
    expect(styles[0]).toBe(oldStyle);
  });
});

describe("swapBodyElement — persist lift", () => {
  it("preserves a persist-id'd element across body replacement", () => {
    document.body.innerHTML = `
      <header><h1>Old Title</h1></header>
      <div ${PERSIST_ATTR}="player" id="kept"><span>state</span></div>
      <main>old content</main>
    `;
    const keptOriginal = document.querySelector("#kept")!;

    const newDoc = htmlDoc(`<!doctype html><html><head></head><body>
      <header><h1>New Title</h1></header>
      <div ${PERSIST_ATTR}="player" id="incoming"></div>
      <main>new content</main>
    </body></html>`);

    swapBodyElement(newDoc.body, document.body);

    // The original DOM node should still be in document — same identity.
    const keptAfter = document.querySelector(`[${PERSIST_ATTR}="player"]`);
    expect(keptAfter).toBe(keptOriginal);
    // It should have replaced the incoming target (positioned where the new
    // marker was), so its parent is the new body and the placeholder #incoming
    // is gone.
    expect(document.querySelector("#incoming")).toBeNull();
    // Surrounding markup is the new content.
    expect(document.querySelector("h1")?.textContent).toBe("New Title");
    expect(document.querySelector("main")?.textContent).toBe("new content");
  });

  it("discards an old persist element when the new body has no matching id", () => {
    document.body.innerHTML = `<div ${PERSIST_ATTR}="orphan">orphan</div>`;
    const newDoc = htmlDoc(`<!doctype html><html><head></head><body><p>fresh</p></body></html>`);

    swapBodyElement(newDoc.body, document.body);

    expect(document.querySelector(`[${PERSIST_ATTR}="orphan"]`)).toBeNull();
    expect(document.querySelector("p")?.textContent).toBe("fresh");
  });

  it("persisted island with data-props: new target WITHOUT data-props → removeAttribute path", () => {
    // old element has data-props; new target is a zfb-island WITHOUT data-props
    document.body.innerHTML = `
      <div ${PERSIST_ATTR}="island1" data-zfb-island data-props='{"count":1}'>island</div>
    `;

    const newDoc = htmlDoc(`<!doctype html><html><head></head><body>
      <div ${PERSIST_ATTR}="island1" data-zfb-island></div>
    </body></html>`);

    swapBodyElement(newDoc.body, document.body);

    const persisted = document.querySelector(`[${PERSIST_ATTR}="island1"]`)!;
    // Branch fired: remount flag should be present
    expect(persisted.hasAttribute("data-zfb-island-remount")).toBe(true);
    // data-props must be removed — NOT written as string "null"
    expect(persisted.hasAttribute("data-props")).toBe(false);
    expect(persisted.getAttribute("data-props")).not.toBe("null");
  });

  it("persisted island WITHOUT data-props: new target WITH data-props → setAttribute path", () => {
    // old element has no data-props; new target is a zfb-island WITH data-props
    document.body.innerHTML = `
      <div ${PERSIST_ATTR}="island2" data-zfb-island>island</div>
    `;

    const newDoc = htmlDoc(`<!doctype html><html><head></head><body>
      <div ${PERSIST_ATTR}="island2" data-zfb-island data-props='{"count":2}'></div>
    </body></html>`);

    swapBodyElement(newDoc.body, document.body);

    const persisted = document.querySelector(`[${PERSIST_ATTR}="island2"]`)!;
    // Branch fired: remount flag should be present
    expect(persisted.hasAttribute("data-zfb-island-remount")).toBe(true);
    // data-props must be set to the new target's value
    expect(persisted.getAttribute("data-props")).toBe('{"count":2}');
  });

  it("persisted island with same data-props on both sides → isSameProps returns true, remount flag not set", () => {
    // Both old and new have identical data-props → branch should NOT fire
    document.body.innerHTML = `
      <div ${PERSIST_ATTR}="island3" data-zfb-island data-props='{"count":3}'>island</div>
    `;

    const newDoc = htmlDoc(`<!doctype html><html><head></head><body>
      <div ${PERSIST_ATTR}="island3" data-zfb-island data-props='{"count":3}'></div>
    </body></html>`);

    swapBodyElement(newDoc.body, document.body);

    const persisted = document.querySelector(`[${PERSIST_ATTR}="island3"]`)!;
    // isSameProps → branch did NOT fire, remount flag must be absent
    expect(persisted.hasAttribute("data-zfb-island-remount")).toBe(false);
    // data-props should still equal the original value
    expect(persisted.getAttribute("data-props")).toBe('{"count":3}');
  });

  it("upgrades declarative shadow-root templates after swap", () => {
    document.body.innerHTML = "";
    const newDoc = htmlDoc(`<!doctype html><html><head></head><body>
      <div id="host"><template shadowrootmode="open"><span>shadow</span></template></div>
    </body></html>`);

    swapBodyElement(newDoc.body, document.body);

    const host = document.querySelector("#host")!;
    expect(host.shadowRoot).not.toBeNull();
    expect(host.shadowRoot?.querySelector("span")?.textContent).toBe("shadow");
    // The <template> should have been removed once attached.
    expect(host.querySelector("template")).toBeNull();
  });
});

describe("saveFocus / restoreFocus", () => {
  it("restores caret position on a persisted <input>", () => {
    document.body.innerHTML = `<form ${PERSIST_ATTR}="form"><input id="i" value="hello"></form>`;
    const input = document.getElementById("i") as HTMLInputElement;
    input.focus();
    input.setSelectionRange(2, 4);

    const restore = saveFocus();

    // Simulate a swap: rebuild the same persisted subtree in place. The
    // saved closure captured the original input reference.
    restore();

    expect(document.activeElement).toBe(input);
    expect(input.selectionStart).toBe(2);
    expect(input.selectionEnd).toBe(4);
  });

  it("does nothing when activeElement is not inside a persist subtree", () => {
    document.body.innerHTML = `<button id="b">click</button>`;
    const btn = document.getElementById("b") as HTMLButtonElement;
    btn.focus();
    expect(document.activeElement).toBe(btn);

    const restore = saveFocus();
    // Move focus elsewhere then call restore — the saved closure should be a
    // no-op (activeElement: null).
    btn.blur();
    restore();
    // Focus should not have been forced back onto the button.
    expect(document.activeElement).not.toBe(btn);
  });

  it("restoreFocus tolerates a null activeElement (no throw)", () => {
    expect(() => restoreFocus({ activeElement: null })).not.toThrow();
  });
});

describe("script re-exec — deselectScripts + detectScriptExecuted", () => {
  it("detectScriptExecuted is true the second time the same src appears", () => {
    const s = document.createElement("script");
    s.src = "/a.js";
    expect(detectScriptExecuted(s)).toBe(false);
    expect(detectScriptExecuted(s)).toBe(true);
  });

  it("deselectScripts marks an already-seen script as executed in the new doc", () => {
    // Build the new doc by hand rather than parseFromString — happy-dom 15.11
    // can still fetch <script src> through the parser even when fileloading
    // settings are disabled. document.implementation.createHTMLDocument() gives
    // us a parserless document that never tries to load anything.
    const newDoc = document.implementation.createHTMLDocument();
    const seenScript = newDoc.createElement("script");
    seenScript.src = "data:application/javascript,/*seen*/";
    newDoc.head.appendChild(seenScript);
    const freshScript = newDoc.createElement("script");
    freshScript.src = "data:application/javascript,/*fresh*/";
    newDoc.head.appendChild(freshScript);

    // Pre-seed the dedup set by visiting /seen.js once.
    detectScriptExecuted(seenScript);

    deselectScripts(newDoc);

    expect(seenScript.dataset["zfbExec"]).toBe("");
    expect(freshScript.dataset["zfbExec"]).toBeUndefined();
  });

  it("data-zfb-rerun bypasses dedup so re-execution proceeds", () => {
    const newDoc = document.implementation.createHTMLDocument();
    const loud = newDoc.createElement("script");
    loud.src = "data:application/javascript,/*loud*/";
    loud.setAttribute("data-zfb-rerun", "");
    newDoc.head.appendChild(loud);

    // Pre-seed the dedup set.
    const seed = newDoc.createElement("script");
    seed.src = "data:application/javascript,/*loud*/";
    detectScriptExecuted(seed);

    deselectScripts(newDoc);

    expect(loud.dataset["zfbExec"]).toBeUndefined();
  });
});

describe("swapRootAttributes", () => {
  it("replaces <html> attributes with the new doc's set", () => {
    document.documentElement.setAttribute("class", "old-theme");
    document.documentElement.setAttribute("lang", "en");
    const newDoc = htmlDoc(
      `<!doctype html><html class="new-theme" lang="ja"><head></head><body></body></html>`,
    );

    swapRootAttributes(newDoc);

    expect(document.documentElement.getAttribute("class")).toBe("new-theme");
    expect(document.documentElement.getAttribute("lang")).toBe("ja");
  });

  it("preserves data-zfb-transition and data-zfb-transition-fallback", () => {
    document.documentElement.setAttribute("data-zfb-transition", "forward");
    document.documentElement.setAttribute("data-zfb-transition-fallback", "old");
    const newDoc = htmlDoc(`<!doctype html><html lang="en"><head></head><body></body></html>`);

    swapRootAttributes(newDoc);

    // Non-overridable attrs reinserted — these belong to the running transition,
    // not to either page's source markup.
    expect(document.documentElement.getAttribute("data-zfb-transition")).toBe("forward");
    expect(document.documentElement.getAttribute("data-zfb-transition-fallback")).toBe("old");
    expect(document.documentElement.getAttribute("lang")).toBe("en");
  });

  it("preserves a consumer attribute listed in the zfb-preserve-html-attrs meta", () => {
    document.head.innerHTML = `<meta name="zfb-preserve-html-attrs" content="data-sidebar-hidden">`;
    document.documentElement.setAttribute("data-sidebar-hidden", "");
    document.documentElement.setAttribute("lang", "en");
    const newDoc = htmlDoc(`<!doctype html><html lang="ja"><head></head><body></body></html>`);

    swapRootAttributes(newDoc);

    // The runtime attribute survives even though the incoming doc lacks it,
    // while non-preserved attrs (lang) still take the new doc's value.
    expect(document.documentElement.hasAttribute("data-sidebar-hidden")).toBe(true);
    expect(document.documentElement.getAttribute("lang")).toBe("ja");
  });

  it("keeps the current runtime value when the incoming doc also sets the attribute", () => {
    document.head.innerHTML = `<meta name="zfb-preserve-html-attrs" content="data-theme">`;
    document.documentElement.setAttribute("data-theme", "dark");
    const newDoc = htmlDoc(
      `<!doctype html><html data-theme="light"><head></head><body></body></html>`,
    );

    swapRootAttributes(newDoc);

    // Preserved attrs are re-applied last, so the live runtime value wins over
    // the server-rendered default.
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("preserves multiple space-separated names and tolerates extra whitespace", () => {
    document.head.innerHTML = `<meta name="zfb-preserve-html-attrs" content="  data-theme   data-sidebar-hidden ">`;
    document.documentElement.setAttribute("data-theme", "dark");
    document.documentElement.setAttribute("data-sidebar-hidden", "");
    const newDoc = htmlDoc(`<!doctype html><html><head></head><body></body></html>`);

    swapRootAttributes(newDoc);

    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.hasAttribute("data-sidebar-hidden")).toBe(true);
  });

  it("normalizes mixed-case preserve-list names to match lowercased DOM attribute names", () => {
    // The DOM lowercases attribute names, so a mixed-case meta entry must still
    // match the live (lowercased) attribute — otherwise the feature silently no-ops.
    document.head.innerHTML = `<meta name="zfb-preserve-html-attrs" content="Data-Theme">`;
    document.documentElement.setAttribute("data-theme", "dark");
    const newDoc = htmlDoc(`<!doctype html><html><head></head><body></body></html>`);

    swapRootAttributes(newDoc);

    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("does not preserve runtime attributes when no preserve-list meta is present", () => {
    document.documentElement.setAttribute("data-sidebar-hidden", "");
    const newDoc = htmlDoc(`<!doctype html><html lang="en"><head></head><body></body></html>`);

    swapRootAttributes(newDoc);

    // No meta → unchanged behavior: the non-preserved runtime attr is dropped.
    expect(document.documentElement.hasAttribute("data-sidebar-hidden")).toBe(false);
    expect(document.documentElement.getAttribute("lang")).toBe("en");
  });
});

describe("swap() composition", () => {
  it("runs deselect + root + head + body + restoreFocus end-to-end", () => {
    document.documentElement.setAttribute("lang", "en");
    document.head.innerHTML = `<title>Old</title>`;
    document.body.innerHTML = `<main>old</main>`;

    const newDoc = htmlDoc(`<!doctype html><html lang="ja"><head>
      <title>New</title>
    </head><body>
      <main>new</main>
    </body></html>`);

    expect(() => swap(newDoc)).not.toThrow();

    expect(document.documentElement.getAttribute("lang")).toBe("ja");
    expect(document.title).toBe("New");
    expect(document.querySelector("main")?.textContent).toBe("new");
  });
});
