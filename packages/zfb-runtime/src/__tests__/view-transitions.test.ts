// Tests for `<ViewTransitions />` — typed no-op contract.
//
// Cross-document View Transitions are opted in via the
// `@view-transition { navigation: auto; }` CSS at-rule on the consumer's
// top-level stylesheet, NOT via this component. The component is kept as
// a typed no-op so existing `<ViewTransitions />` mounts compile unchanged.
//
// Previously this file held ~22 tests covering meta-tag SSR shape, inline
// IIFE SSR shape, and a happy-dom click-intercept smoke suite (modifier
// keys, target=_blank, mailto:/tel:/javascript:, hash-only, cross-origin,
// download attr, default-prevented events, happy-path interception, etc.).
// All of that machinery is gone. The contract is now: `ViewTransitions()`
// returns `[]` (empty readonly array of structural VNodes).
//
// The intentionally minimal test surface here is the new SSR contract plus
// a TS-level assertion that the public type signature is preserved.

import { describe, expect, it } from "vitest";

import { ViewTransitions, type ViewTransitionsElement } from "../view-transitions.js";

describe("ViewTransitions — typed no-op contract", () => {
  it("returns an empty array (no DOM injection)", () => {
    expect(ViewTransitions()).toEqual([]);
  });

  it("returns a stable readonly array shape", () => {
    const fragment = ViewTransitions();
    expect(Array.isArray(fragment)).toBe(true);
    expect(fragment).toHaveLength(0);
  });

  it("preserves the public type signature () => readonly ViewTransitionsElement[]", () => {
    // TS-level contract: this satisfies-check would fail at compile time
    // if `ViewTransitions` ever stopped returning `readonly
    // ViewTransitionsElement[]`. The runtime check is incidental — the
    // value of this test is that `tsc --noEmit` enforces the shape.
    const _typeCheck: () => readonly ViewTransitionsElement[] = ViewTransitions;
    expect(typeof _typeCheck).toBe("function");
  });
});
