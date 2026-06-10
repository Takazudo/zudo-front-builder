/**
 * Preact strict-mode type fixture.
 *
 * Purpose: verify that Island accepts Preact's ComponentChildren, VNode, and
 * JSX.Element at every input boundary (children + ssrFallback) with zero
 * `as unknown as` casts.
 *
 * This file is compiled by `tsc -p tsconfig.preact-fixture.json` (noEmit,
 * jsxImportSource preact, strict). It is intentionally excluded from the main
 * tsconfig and from vitest's test glob — it is a compile-only type check.
 */

import type { ComponentChildren, JSX, VNode as PreactVNode } from "preact";
import { Island } from "../src/island.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface ButtonProps {
  label: string;
  count: number;
}

function Button(_props: ButtonProps) {
  return <button>{_props.label}</button>;
}

// ---------------------------------------------------------------------------
// 1. Inline JSX as children
// ---------------------------------------------------------------------------

export function inlineJsxChild() {
  return (
    <Island when="load">
      <Button label="click" count={0} />
    </Island>
  );
}

// ---------------------------------------------------------------------------
// 2. ComponentChildren forwarded as children
// ---------------------------------------------------------------------------

export function componentChildrenAsChildren(cc: ComponentChildren) {
  return <Island when="idle">{cc}</Island>;
}

// ---------------------------------------------------------------------------
// 3. ComponentChildren forwarded as ssrFallback
// ---------------------------------------------------------------------------

export function componentChildrenAsSsrFallback(cc: ComponentChildren) {
  return (
    <Island ssrFallback={cc}>
      <Button label="heavy" count={0} />
    </Island>
  );
}

// ---------------------------------------------------------------------------
// 4. PreactVNode<interface-props> as ssrFallback
// ---------------------------------------------------------------------------

export function preactVNodeAsssrFallback(vnode: PreactVNode<ButtonProps>) {
  return (
    <Island ssrFallback={vnode}>
      <Button label="heavy" count={0} />
    </Island>
  );
}

// ---------------------------------------------------------------------------
// 5. JSX.Element[] as children
// ---------------------------------------------------------------------------

export function jsxElementArrayAsChildren(items: JSX.Element[]) {
  return <Island when="visible">{items}</Island>;
}

// ---------------------------------------------------------------------------
// 6. Island return used in ComponentChildren slot and JSX embedding
// ---------------------------------------------------------------------------

export function islandReturnInComponentChildren(): ComponentChildren {
  // Island returns IslandElement (a plain object with type/props/key).
  // Assigning it into a ComponentChildren slot exercises the return direction.
  const el = (
    <Island when="load">
      <Button label="a" count={1} />
    </Island>
  );
  // Embed directly as JSX child — valid because IslandElement is an object.
  return <div>{el}</div>;
}

// ---------------------------------------------------------------------------
// 7. Negative control — symbol is not assignable (should @ts-expect-error)
// ---------------------------------------------------------------------------

export function negativeControlSymbol(sym: symbol) {
  // @ts-expect-error — symbol is not part of VNode / ComponentChildren
  return <Island>{sym}</Island>;
}
