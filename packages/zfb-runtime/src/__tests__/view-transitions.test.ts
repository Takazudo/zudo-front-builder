// Tests for `<ViewTransitions />` — typed no-op contract.
//
// Cross-document View Transitions are opted in via the
// `@view-transition { navigation: auto; }` CSS at-rule on the consumer's
// top-level stylesheet, NOT via this component. The component is kept as a
// typed no-op so existing `<ViewTransitions />` mounts in downstream code
// compile unchanged. Per the W3D port (zudolab/zudo-doc#1519) the inline IIFE
// that the component used to emit was deleted; <ClientRouter /> is now the
// SPA opt-in. This test pins the back-compat contract: ViewTransitions() must
// return `[]` so the JSX call site emits no DOM.
//
// Previously this file held ~18 click-intercept smoke tests against the
// deleted IIFE. They are gone — the click/submit intercept is now exercised
// by `__tests__/client-router/router.test.ts`.

import { describe, expect, it } from "vitest";

import { ViewTransitions, type ViewTransitionsElement } from "../view-transitions.js";

describe("ViewTransitions — typed no-op back-compat contract", () => {
  it("returns an empty array so existing JSX mounts emit no DOM", () => {
    const fragment = ViewTransitions();
    expect(fragment).toEqual([]);
    // The TS signature `() => readonly ViewTransitionsElement[]` is enforced
    // at compile time by `pnpm typecheck`; the runtime check below is the
    // observable half of that contract.
    const _typeCheck: () => readonly ViewTransitionsElement[] = ViewTransitions;
    expect(typeof _typeCheck).toBe("function");
  });
});
