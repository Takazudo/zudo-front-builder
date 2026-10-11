/**
 * @vitest-environment happy-dom
 */
// L2 (happy-dom) checks for captureDocumentLoads() — the shared `location`
// shim every router suite leans on. It must record full-document loads
// without freezing the URL, and hand `location` back to happy-dom exactly as
// it found it so a later test's History writes still reflect into
// `location.href` (#4130).

import { afterEach, describe, expect, it, vi } from "vite-plus/test";

import { captureDocumentLoads, installHappyDomShim, type DocumentLoadCapture } from "./_helpers.js";

installHappyDomShim();

const ownHref = () => Object.getOwnPropertyDescriptor(location, "href");
const ownReplace = () => Object.getOwnPropertyDescriptor(location, "replace");
const nativeReplace = location.replace;

let loads: DocumentLoadCapture | undefined;
afterEach(() => {
  loads?.restore();
  loads = undefined;
});

describe("captureDocumentLoads() — live reads", () => {
  it("reports pushState/replaceState moves through the shimmed href", () => {
    loads = captureDocumentLoads();

    history.pushState(null, "", "/pushed");
    expect(new URL(location.href).pathname).toBe("/pushed");

    history.replaceState(null, "", "/replaced");
    expect(new URL(location.href).pathname).toBe("/replaced");
    expect(loads.assigned).toEqual([]);
    expect(loads.replaced).toEqual([]);
  });
});

describe("captureDocumentLoads() — load capture", () => {
  it("records href assignments and replace() calls without moving the URL", () => {
    loads = captureDocumentLoads();
    const before = location.href;

    location.href = "/assigned-one";
    location.href = new URL("/assigned-two", before).href;
    location.replace("/replaced-one");
    location.replace(new URL("/replaced-two", before));

    expect(loads.assigned).toEqual(["/assigned-one", new URL("/assigned-two", before).href]);
    expect(loads.replaced).toEqual(["/replaced-one", new URL("/replaced-two", before).href]);
    expect(location.href).toBe(before);
  });
});

describe("captureDocumentLoads() — restore", () => {
  it("removes the own properties again when none existed before", () => {
    expect(ownHref()).toBeUndefined();
    expect(ownReplace()).toBeUndefined();

    loads = captureDocumentLoads();
    expect(ownHref()).toBeDefined();
    expect(ownReplace()).toBeDefined();

    loads.restore();
    expect(ownHref()).toBeUndefined();
    expect(ownReplace()).toBeUndefined();
    expect(location.replace).toBe(nativeReplace);
  });

  it("reinstates a pre-existing own descriptor instead of deleting it", () => {
    const frozenGet = () => "https://frozen.test/own-href";
    const frozenSet = () => {};
    const priorReplace = () => {};
    const priorHrefDescriptor = {
      configurable: true,
      enumerable: true,
      get: frozenGet,
      set: frozenSet,
    };
    const priorReplaceDescriptor = {
      configurable: true,
      enumerable: true,
      writable: false,
      value: priorReplace,
    };
    Object.defineProperty(location, "href", priorHrefDescriptor);
    Object.defineProperty(location, "replace", priorReplaceDescriptor);
    try {
      loads = captureDocumentLoads();
      expect(ownHref()?.get).not.toBe(frozenGet);
      expect(location.replace).not.toBe(priorReplace);

      loads.restore();
      expect(ownHref()).toEqual(priorHrefDescriptor);
      expect(location.href).toBe("https://frozen.test/own-href");
      expect(ownReplace()).toEqual(priorReplaceDescriptor);
    } finally {
      delete (location as unknown as Record<string, unknown>)["href"];
      delete (location as unknown as Record<string, unknown>)["replace"];
    }
  });

  it("rolls the href shim back if installing replace fails", () => {
    const priorHrefDescriptor = ownHref();
    const priorReplaceDescriptor = ownReplace();
    const nativeDefineProperty = Object.defineProperty;
    const installError = new Error("replace installation failed");
    const definePropertySpy = vi
      .spyOn(Object, "defineProperty")
      .mockImplementation((target, property, descriptor) => {
        if (target === location && property === "replace") throw installError;
        return nativeDefineProperty(target, property, descriptor);
      });

    try {
      expect(() => captureDocumentLoads()).toThrow(installError);
      expect(ownHref()).toEqual(priorHrefDescriptor);
      expect(ownReplace()).toEqual(priorReplaceDescriptor);
      history.replaceState(null, "", "/after-install-failure");
      expect(new URL(location.href).pathname).toBe("/after-install-failure");
    } finally {
      definePropertySpy.mockRestore();
    }
  });

  it("is idempotent: a second restore() is a no-op", () => {
    loads = captureDocumentLoads();
    loads.restore();
    expect(ownHref()).toBeUndefined();

    // A later capture installs fresh own properties; the stale restore() must
    // not tear them down.
    const later = captureDocumentLoads();
    loads.restore();
    expect(ownHref()).toBeDefined();
    location.href = "/still-captured";
    expect(later.assigned).toEqual(["/still-captured"]);
    later.restore();
    expect(ownHref()).toBeUndefined();
  });
});

describe("captureDocumentLoads() — the next test starts clean", () => {
  it("leaves a capture for afterEach to restore", () => {
    loads = captureDocumentLoads();
    location.href = "/leaked?";
    expect(loads.assigned).toEqual(["/leaked?"]);
  });

  it("sees happy-dom's prototype getter again after the previous test", () => {
    expect(ownHref()).toBeUndefined();
    expect(ownReplace()).toBeUndefined();
    history.replaceState(null, "", "/after-restore");
    expect(new URL(location.href).pathname).toBe("/after-restore");
  });
});
