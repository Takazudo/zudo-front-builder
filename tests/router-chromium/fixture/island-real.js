// REAL (non-stub) module for @takazudo/zfb/runtime — used only by the
// island-a.html / island-b.html fixture pair. Every other fixture page still
// maps this specifier to the no-op island-stubs.js; that stub is precisely
// the blind spot that hid the pre-#1389 persist bug, since a no-op cannot
// observe whether it was called correctly. This module actually mounts and
// unmounts a tiny counter island, mirroring packages/zfb/src/runtime.ts's
// mount/unmount contract closely enough to prove the router's persist
// handshake against genuine side effects. See island-persist.chromium.spec.mjs.
const PERSIST_ATTR = "data-zfb-transition-persist";

// element -> cleanup thunk, mirrors zfb/runtime.ts's module-private `mounted`
// WeakMap<Element, () => void>.
const mounted = new WeakMap();

// Debug counters the spec asserts on directly.
window.__islandDebug = { mounts: 0, unmounts: 0 };

function mountCounter(el) {
  if (mounted.has(el)) return;
  let count = 0;
  const out = el.querySelector("[data-counter-value]");
  const btn = el.querySelector("[data-counter-inc]");
  out.textContent = String(count);
  const onClick = () => {
    count += 1;
    out.textContent = String(count);
  };
  btn.addEventListener("click", onClick);
  window.__islandDebug.mounts++;
  mounted.set(el, () => {
    btn.removeEventListener("click", onClick);
    window.__islandDebug.unmounts++;
  });
}

export function cancelPendingIslands() {}

export function mountNewIslands() {
  for (const el of document.querySelectorAll("[data-counter-island]")) {
    mountCounter(el);
  }
}

// Signature mirrors zfb/runtime.ts's unmountIslands(root, incomingBody) exactly
// (issue #1389): with no incomingBody, nothing is preserved and every counter
// is unmounted — the pre-#1389 behavior. With incomingBody, a counter whose
// persist id also appears there is left mounted, because swapBodyElement will
// physically lift that node into the new body rather than discard it.
export function unmountIslands(root = document.body, incomingBody) {
  const preserved = new Set();
  if (incomingBody) {
    for (const el of incomingBody.querySelectorAll(`[${PERSIST_ATTR}]`)) {
      const id = el.getAttribute(PERSIST_ATTR);
      if (id) preserved.add(id);
    }
  }
  for (const el of root.querySelectorAll("[data-counter-island]")) {
    const id = el.getAttribute(PERSIST_ATTR);
    if (id && preserved.has(id)) continue;
    const cleanup = mounted.get(el);
    if (cleanup) {
      cleanup();
      mounted.delete(el);
    }
  }
}
