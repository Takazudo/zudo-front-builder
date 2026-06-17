/// <reference lib="dom" />
/// <reference lib="dom.iterable" />
// Ported verbatim from Astro transitions/swap-functions.ts. Mechanical renames
// applied per W1B §13.4 (data-astro-* → data-zfb-*, dataset.astro* → dataset.zfb*,
// astro-island element-name branch → data-zfb-island marker per W1B §12.3 island-lifecycle
// contract). vueScopedStyleId is intentionally KEPT verbatim per W2A §2.8 (the W3B
// issue body's "DELETE" instruction is overridden by W2A's reclassification).

export type SavedFocus = {
  activeElement: HTMLElement | null;
  start?: number | null;
  end?: number | null;
};

const PERSIST_ATTR = "data-zfb-transition-persist";

const NON_OVERRIDABLE_ZFB_ATTRS = ["data-zfb-transition", "data-zfb-transition-fallback"];

// Consumers extend the preserve-set via <meta name="zfb-preserve-html-attrs"
// content="data-theme data-sidebar-hidden …">, emitted by
// <ClientRouter preserveHtmlAttrs={[…]} />. Without it, a runtime <html>
// attribute set from a persisted island is wiped on every swap. See
// client-router/port-spec.md deviation #11 and Takazudo/zudo-front-builder#1103.
const PRESERVE_ATTRS_META_NAME = "zfb-preserve-html-attrs";

const knownVueScopedStyles = new Map<string, HTMLStyleElement>();

const scriptsAlreadyRan = new Set<string>();
export function detectScriptExecuted(script: HTMLScriptElement) {
  const key = script.src ? new URL(script.src, location.href).href : script.textContent!;
  if (scriptsAlreadyRan.has(key)) return true;
  scriptsAlreadyRan.add(key);
  return false;
}

/*
 * 	Mark new scripts that should not execute
 */
export function deselectScripts(doc: Document) {
  for (const s2 of doc.scripts) {
    if (
      // Check if the script should be rerun regardless of it being the same
      !s2.hasAttribute("data-zfb-rerun") &&
      // Check if the script has already been executed
      detectScriptExecuted(s2)
    ) {
      // the old script is in the new document and doesn't have the rerun attribute
      // we mark it as executed to prevent re-execution
      s2.dataset["zfbExec"] = "";
    }
  }
}

/*
 * Read the consumer-configured preserve-list from the live document's
 * <meta name="zfb-preserve-html-attrs"> tag. The list is a site-wide, static
 * contract (the tag is rendered on every page, so it is present on both the
 * current and incoming documents); reading the live document is sufficient.
 * Returns [] when the tag is absent.
 */
function consumerPreservedAttrs(): string[] {
  const content = document
    .querySelector(`meta[name="${PRESERVE_ATTRS_META_NAME}"]`)
    ?.getAttribute("content");
  return content ? content.split(/\s+/).filter(Boolean) : [];
}

/*
 * swap attributes of the html element
 * delete all attributes from the current document
 * insert all attributes from doc
 * reinsert all original attributes whose name is in the preserve-set:
 * NON_OVERRIDABLE_ZFB_ATTRS (transition-internal) ∪ the consumer preserve-list.
 * Preserved attributes are re-applied last, so a runtime value on the current
 * root wins over the incoming document's default (e.g. a localStorage-driven
 * data-theme). See client-router/port-spec.md deviation #11 / #1103.
 */
export function swapRootAttributes(newDoc: Document) {
  const currentRoot = document.documentElement;
  const preserve = new Set([...NON_OVERRIDABLE_ZFB_ATTRS, ...consumerPreservedAttrs()]);
  const preservedAttributes = [...currentRoot.attributes].filter(
    ({ name }) => (currentRoot.removeAttribute(name), preserve.has(name)),
  );
  [...newDoc.documentElement.attributes, ...preservedAttributes].forEach(({ name, value }) =>
    currentRoot.setAttribute(name, value),
  );
}

/*
 * make the old head look like the new one
 */
export function swapHeadElements(doc: Document) {
  for (const el of Array.from(document.head.children)) {
    const newEl = persistedHeadElement(el as HTMLElement, doc);
    // If the element exists in the document already, remove it
    // from the new document and leave the current node alone
    if (newEl) {
      newEl.remove();
    } else {
      if ((import.meta as any).env?.DEV && el instanceof HTMLStyleElement) {
        // In DEV mode, keep updated Vue scoped styles for later reuse
        const viteDevId = vueScopedStyleId(el);
        viteDevId && knownVueScopedStyles.set(viteDevId, el);
      }
      // If the element does not exist in the new document, remove the element from current the head.
      el.remove();
    }
  }

  // Everything left in the new head is new, append it all.
  if ((import.meta as any).env?.DEV) {
    // In DEV mode, replace known Vue scoped styles with the versions we remembered
    [...doc.head.children].forEach((child) => {
      document.head.append(knownVueScopedStyles.get((child as any).dataset?.viteDevId) || child);
    });
  } else {
    document.head.append(...doc.head.children);
  }
}

export function swapBodyElement(newElement: Element, oldElement: Element) {
  // Lift persist elements to <html> before the body swap so they stay in the DOM
  // throughout replaceWith(). This prevents Safari from losing WebGL context on
  // <canvas> elements due to brief DOM detachment. Uses moveBefore() where available
  // (Chrome 133+) for zero-detachment atomic moves.
  const persistPairs: { old: Element; newTarget: Element }[] = [];
  const docEl = oldElement.ownerDocument.documentElement;

  // moveBefore() is not yet in TypeScript's DOM lib, feature-detect and wrap.
  const moveBefore: ((parent: Node, node: Node, child: Node | null) => void) | null =
    typeof (docEl as any).moveBefore === "function"
      ? (parent, node, child) => (parent as any).moveBefore(node, child)
      : null;

  for (const el of oldElement.querySelectorAll(`[${PERSIST_ATTR}]`)) {
    const id = el.getAttribute(PERSIST_ATTR);
    const newEl = newElement.querySelector(`[${PERSIST_ATTR}="${id}"]`);
    if (!newEl) continue; // no matching target — leave in old body to be discarded
    persistPairs.push({ old: el, newTarget: newEl });
    if (moveBefore) {
      moveBefore(docEl, el, null);
    } else {
      docEl.appendChild(el);
    }
  }

  // this will reset scroll Position
  oldElement.replaceWith(newElement);

  // Move persist elements into the new body at the position of their targets
  for (const { old: el, newTarget } of persistPairs) {
    if (moveBefore) {
      moveBefore(newTarget.parentNode!, el, newTarget);
      newTarget.remove();
    } else {
      newTarget.replaceWith(el);
    }
    // For islands, copy over the props to allow them to re-render
    if (
      newTarget.matches("[data-zfb-island]") &&
      shouldCopyProps(el as HTMLElement) &&
      !isSameProps(el, newTarget)
    ) {
      el.setAttribute("ssr", "");
      // zfb island wrapper writes SSR props to data-props, not props (different attribute, not just renamed).
      const np = newTarget.getAttribute("data-props");
      if (np !== null) el.setAttribute("data-props", np);
      else el.removeAttribute("data-props");
    }
  }

  // This will upgrade any Declarative Shadow DOM in the new body.
  attachShadowRoots(newElement);
}

/**
 * Attach Shadow DOM roots for templates with the declarative `shadowrootmode` attribute.
 * @see https://web.dev/articles/declarative-shadow-dom#polyfill
 * @param root DOM subtree to attach shadow roots within.
 */
function attachShadowRoots(root: Element | ShadowRoot) {
  root.querySelectorAll<HTMLTemplateElement>("template[shadowrootmode]").forEach((template) => {
    const mode = template.getAttribute("shadowrootmode");
    const parent = template.parentNode;
    if ((mode === "closed" || mode === "open") && parent instanceof HTMLElement) {
      // Skip if shadow root already exists (e.g., from transition-persisted elements)
      if (parent.shadowRoot) {
        template.remove();
        return;
      }
      const shadowRoot = parent.attachShadow({ mode });
      shadowRoot.appendChild(template.content);
      template.remove();
      attachShadowRoots(shadowRoot);
    }
  });
}

export const saveFocus = (): (() => void) => {
  const activeElement = document.activeElement as HTMLElement;
  // The element that currently has the focus is part of a DOM tree
  // that will survive the transition to the new document.
  // Save the element and the cursor position
  if (activeElement?.closest(`[${PERSIST_ATTR}]`)) {
    if (activeElement instanceof HTMLInputElement || activeElement instanceof HTMLTextAreaElement) {
      const start = activeElement.selectionStart;
      const end = activeElement.selectionEnd;
      return () => restoreFocus({ activeElement, start, end });
    }
    return () => restoreFocus({ activeElement });
  } else {
    return () => restoreFocus({ activeElement: null });
  }
};

export const restoreFocus = ({ activeElement, start, end }: SavedFocus) => {
  if (activeElement) {
    activeElement.focus();
    if (activeElement instanceof HTMLInputElement || activeElement instanceof HTMLTextAreaElement) {
      if (typeof start === "number") activeElement.selectionStart = start;
      if (typeof end === "number") activeElement.selectionEnd = end;
    }
  }
};

export const vueScopedStyleId = (el: HTMLStyleElement): string => {
  const viteDevId = el.dataset["viteDevId"] || "";

  const url = new URL(viteDevId, location.href);
  return url.searchParams.get("vue") !== null &&
    url.searchParams.get("type") === "style" &&
    url.searchParams.has("scoped")
    ? viteDevId
    : "";
};

// Check for a head element that should persist and returns it,
// either because it has the data attribute or because replacing it would cause avoidable FOUC.
const persistedHeadElement = (el: HTMLElement, newDoc: Document): Element | null => {
  const id = el.getAttribute(PERSIST_ATTR);
  const newEl = id && newDoc.head.querySelector(`[${PERSIST_ATTR}="${id}"]`);
  if (newEl) {
    return newEl;
  }
  if (el.matches("link[rel=stylesheet]")) {
    const href = el.getAttribute("href");
    return newDoc.head.querySelector(`link[rel=stylesheet][href="${href}"]`);
  }
  // In dev mode, Vite injects <style data-vite-dev-id="..."> elements whose
  // textContent may later be transformed (especially Vue's `:deep()` → `[data-v-xxx]`).
  // Match these by their stable dev ID so the already-transformed style is preserved
  // across ClientRouter soft navigations instead of being replaced by the raw version.
  // There are other ids that can't be preserved and need a refresh, like Uno's /__uno.css,
  // which keeps the same id, but with different contents.
  // To avoid enumerating all exceptions, we only apply the auto-persist logic to elements
  // that look like Vue's dev styles.
  if ((import.meta as any).env?.DEV && el instanceof HTMLStyleElement) {
    const viteDevId = vueScopedStyleId(el);
    if (viteDevId) {
      return newDoc.head.querySelector(`style[data-vite-dev-id="${viteDevId}"]`);
    }
  }
  // Preserve inline <style> elements with identical content across navigations.
  // This prevents unnecessary removal and re-insertion of styles (e.g. @font-face
  // declarations from <Font>), which would cause the browser to re-evaluate them
  // and trigger a flash of unstyled text (FOUT).
  if (el.tagName === "STYLE" && el.textContent) {
    const styles = newDoc.head.querySelectorAll("style");
    for (const s of styles) {
      if (s.textContent === el.textContent) {
        return s;
      }
    }
  }
  // Preserve font preload links across navigations to avoid re-fetching cached fonts.
  if (el.matches("link[rel=preload][as=font]")) {
    const href = el.getAttribute("href");
    return newDoc.head.querySelector(`link[rel=preload][as=font][href="${href}"]`);
  }
  return null;
};

const shouldCopyProps = (el: HTMLElement): boolean => {
  const persistProps = el.dataset["zfbTransitionPersistProps"];
  return persistProps == null || persistProps === "false";
};

const isSameProps = (oldEl: Element, newEl: Element) => {
  // zfb island wrapper writes SSR props to data-props, not props (different attribute, not just renamed).
  return oldEl.getAttribute("data-props") === newEl.getAttribute("data-props");
};

export const swapFunctions = {
  deselectScripts,
  swapRootAttributes,
  swapHeadElements,
  swapBodyElement,
  saveFocus,
};

export const swap = (doc: Document) => {
  deselectScripts(doc);
  swapRootAttributes(doc);
  swapHeadElements(doc);
  const restoreFocusFunction = saveFocus();
  swapBodyElement(doc.body, document.body);
  restoreFocusFunction();
};
