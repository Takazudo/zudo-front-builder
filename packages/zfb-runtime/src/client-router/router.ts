/// <reference lib="dom" />
/// <reference lib="dom.iterable" />
// Ported from Astro transitions/router.ts (transition orchestration half).
// Source: https://raw.githubusercontent.com/withastro/astro/main/packages/astro/src/transitions/router.ts
// Issue: zudolab/zudo-doc#1516 (W3C1), parent epic zudolab/zudo-doc#1510.
//
// Mechanical renames per W1B §13.5:
//   astro:* event names         → zfb:*
//   data-astro-* attributes     → data-zfb-*
//   astro-view-transitions-*    → zfb-view-transitions-*
//   .astro-route-announcer      → .zfb-route-announcer
//   dataset.astroExec           → dataset.zfbExec
//   dataset.astroHistory        → dataset.zfbHistory
//   dataset.astroRerun          → dataset.zfbRerun
//
// Named-cause deviations:
//   - `internalFetchHeaders` import dropped; replaced with `{}` (W1B §13.5 — zfb adapters
//     do not currently expose a per-fetch internal-headers contract).
//   - `prepareForClientOnlyComponents` dropped entirely (W1B §13.5 / W3C2 — DEV-only
//     iframe trick that compensates for Vite per-component CSS injection on hydrate;
//     not applicable to zfb islands which inject CSS via the bundle).
//   - `import.meta.env.SSR` access uses `(import.meta as any).env?.SSR` (no Vite
//     ambient types in zfb-runtime tsconfig — same workaround used in W3B).
//   - `inBrowser` evaluates to `typeof document !== "undefined"` rather than relying
//     on the SSR flag, because the runtime package serves both server- and client-side
//     code; same observable behavior in browser and on SSR.
//   - `announce()` is a TODO stub (W3C3 owns the route announcer).
//
// W3C2 additions (this file):
//   - `navigate()` public entry.
//   - `onPopState`, `onScrollEnd`.
//   - Top-level `if (inBrowser)` initialization block (seeds `currentHistoryIndex`
//     from `history.state`, registers popstate / load / scrollend listeners, and
//     marks already-executed scripts with `dataset["zfbExec"] = ""`).
//
// W3C1 deferred to W3C3:
//   - `announce()` route-announcer implementation.
//   - Click + form intercept.

import {
  doPreparation,
  doSwap,
  onPageLoad,
  triggerEvent,
  updateScrollPosition,
  type TransitionBeforePreparationEvent,
} from "./events.js";
import { detectScriptExecuted } from "./swap-functions.js";
import type { Direction, Fallback, Options } from "./types.js";
// Island re-bootstrap and deferred-cancel after body swap (W1B §12.2, §12.5).
// mountNewIslands() is called after runScripts() and before onPageLoad().
// cancelPendingIslands() is called before doSwap() so deferred callbacks
// (rIC/IntersectionObserver) do not fire against orphan elements.
import { cancelPendingIslands, mountNewIslands, unmountIslands } from "@takazudo/zfb/runtime";

type State = {
  index: number;
  scrollX: number;
  scrollY: number;
};
type Navigation = { controller: AbortController };
type Transition = {
  // The view transitions object (API and simulation)
  viewTransition?: ViewTransition;
  // Simulation: Whether transition was skipped
  transitionSkipped: boolean;
  // Simulation: The resolve function of the finished promise
  viewTransitionFinished?: () => void;
};

// Adapter-specific internal fetch headers. zfb adapters do not currently expose this
// contract — the empty object preserves the call-shape so the loop in fetchHTML is a
// no-op until/unless an adapter wires this up.
const internalFetchHeaders: Record<string, string> = {};

// Detect browser context. Astro uses `import.meta.env.SSR === false` (Vite-injected).
// The zfb-runtime tsconfig has no Vite ambient types; checking for `document` is
// behaviorally identical and avoids a Vite type dependency.
const inBrowser = typeof document !== "undefined";

export const supportsViewTransitions = inBrowser && !!document.startViewTransition;

export const transitionEnabledOnThisPage = () =>
  inBrowser && !!document.querySelector('[name="zfb-view-transitions-enabled"]');

const samePage = (thisLocation: URL, otherLocation: URL) =>
  thisLocation.pathname === otherLocation.pathname && thisLocation.search === otherLocation.search;

// The previous navigation that might still be in processing
let mostRecentNavigation: Navigation | undefined;
// The previous transition that might still be in processing
let mostRecentTransition: Transition | undefined;
// When we traverse the history, the window.location is already set to the new location.
// This variable tells us where we came from
let originalLocation: URL;

// Route announcer — ported from Astro's announce(). Creates (or reuses) a single
// shared aria-live <div> per navigation, so screen readers announce the new page title.
// The 60ms delay is Astro's magic number: screen readers need to see the element change
// and may miss it if it happens too quickly.
const announce = () => {
  let div = document.createElement("div");
  div.setAttribute("aria-live", "assertive");
  div.setAttribute("aria-atomic", "true");
  div.className = "zfb-route-announcer";
  document.body.append(div);
  setTimeout(
    () => {
      let title = document.title || document.querySelector("h1")?.textContent || location.pathname;
      div.textContent = title;
    },
    // Screen readers need to see the element change; 60ms is Astro's empirically chosen delay.
    60,
  );
};

const PERSIST_ATTR = "data-zfb-transition-persist";
const DIRECTION_ATTR = "data-zfb-transition";
const OLD_NEW_ATTR = "data-zfb-transition-fallback";

let parser: DOMParser;

// The History API does not tell you if navigation is forward or back, so
// you can figure it using an index. On pushState the index is incremented so you
// can use that to determine popstate if going forward or back.
let currentHistoryIndex = 0;

if (inBrowser) {
  if (history.state) {
    // Here we reloaded a page with history state
    // (e.g. history navigation from non-transition page or browser reload)
    currentHistoryIndex = history.state.index;
    scrollTo({ left: history.state.scrollX, top: history.state.scrollY });
  } else if (transitionEnabledOnThisPage()) {
    // This page is loaded from the browser address bar or via a link from extern,
    // it needs a state in the history
    history.replaceState({ index: currentHistoryIndex, scrollX, scrollY }, "");
    history.scrollRestoration = "manual";
  }
}

// returns the contents of the page or null if the router can't deal with it.
async function fetchHTML(
  href: string,
  init?: RequestInit,
): Promise<null | { html: string; redirected?: string; mediaType: DOMParserSupportedType }> {
  try {
    // Apply adapter-specific headers for internal fetches
    const headers = new Headers(init?.headers);
    for (const [key, value] of Object.entries(internalFetchHeaders) as [string, string][]) {
      headers.set(key, value);
    }
    const res = await fetch(href, { ...init, headers });
    const contentType = res.headers.get("content-type") ?? "";
    // drop potential charset (+ other name/value pairs) as parser needs the mediaType
    const mediaType = contentType.split(";", 1)[0]!.trim();
    // the DOMParser can handle two types of HTML
    if (mediaType !== "text/html" && mediaType !== "application/xhtml+xml") {
      // everything else (e.g. audio/mp3) will be handled by the browser but not by us
      return null;
    }
    const html = await res.text();
    // exactOptionalPropertyTypes: true forbids assigning `undefined` to a `redirected?: string`
    // slot, so omit the property when not redirected instead of setting it to undefined.
    return res.redirected ? { html, redirected: res.url, mediaType } : { html, mediaType };
  } catch {
    // can't fetch, let someone else deal with it.
    return null;
  }
}

export function getFallback(): Fallback {
  const el = document.querySelector('[name="zfb-view-transitions-fallback"]');
  if (el) {
    return el.getAttribute("content") as Fallback;
  }
  return "animate";
}

function runScripts() {
  let wait = Promise.resolve();
  let needsWaitForInlineModuleScript = false;
  // The original code made the assumption that all inline scripts are directly executed when inserted into the DOM.
  // This is not true for inline module scripts, which are deferred but still executed in order.
  // inline module scripts cannot be awaited for with onload.
  // Thus to be able to wait for the execution of all scripts, we make sure that the last inline module script
  // is always followed by an external module script
  for (const script of document.getElementsByTagName("script")) {
    script.dataset["zfbExec"] === undefined &&
      script.getAttribute("type") === "module" &&
      (needsWaitForInlineModuleScript = script.getAttribute("src") === null);
  }
  needsWaitForInlineModuleScript &&
    document.body.insertAdjacentHTML(
      "beforeend",
      `<script type="module" src="data:application/javascript,"/>`,
    );

  for (const script of document.getElementsByTagName("script")) {
    if (script.dataset["zfbExec"] === "") continue;
    const type = script.getAttribute("type");
    if (type && type !== "module" && type !== "text/javascript") continue;
    const newScript = document.createElement("script");
    newScript.innerHTML = script.innerHTML;
    for (const attr of script.attributes) {
      if (attr.name === "src") {
        const p = new Promise((r) => {
          newScript.onload = newScript.onerror = r;
        });
        wait = wait.then(() => p as any);
      }
      newScript.setAttribute(attr.name, attr.value);
    }
    newScript.dataset["zfbExec"] = "";
    script.replaceWith(newScript);
  }
  return wait;
}

// Add a new entry to the browser history. This also sets the new page in the browser address bar.
// Sets the scroll position according to the hash fragment of the new location.
const moveToLocation = (
  to: URL,
  from: URL,
  options: Options,
  pageTitleForBrowserHistory: string,
  historyState?: State,
) => {
  const intraPage = samePage(from, to);

  const targetPageTitle = document.title;
  document.title = pageTitleForBrowserHistory;

  let scrolledToTop = false;
  if (to.href !== location.href && !historyState) {
    if (options.history === "replace") {
      const current = history.state;
      history.replaceState(
        {
          ...options.state,
          index: current.index,
          scrollX: current.scrollX,
          scrollY: current.scrollY,
        },
        "",
        to.href,
      );
    } else {
      history.pushState(
        { ...options.state, index: ++currentHistoryIndex, scrollX: 0, scrollY: 0 },
        "",
        to.href,
      );
    }
  }
  document.title = targetPageTitle;
  // now we are on the new page for non-history navigation!
  // (with history navigation page change happens before popstate is fired)
  originalLocation = to;

  // freshly loaded pages start from the top
  if (!intraPage) {
    scrollTo({ left: 0, top: 0, behavior: "instant" });
    scrolledToTop = true;
  }

  if (historyState) {
    scrollTo(historyState.scrollX, historyState.scrollY);
  } else {
    if (to.hash) {
      // because we are already on the target page ...
      // ... what comes next is an intra-page navigation
      // that won't reload the page but instead scroll to the fragment
      history.scrollRestoration = "auto";
      const savedState = history.state;
      location.href = to.href; // this kills the history state on Firefox
      if (!history.state) {
        history.replaceState(savedState, ""); // this restores the history state
        if (intraPage) {
          window.dispatchEvent(new PopStateEvent("popstate"));
        }
      }
    } else {
      if (!scrolledToTop) {
        scrollTo({ left: 0, top: 0, behavior: "instant" });
      }
    }
    history.scrollRestoration = "manual";
  }
};

function preloadStyleLinks(newDocument: Document) {
  const links: Promise<any>[] = [];
  for (const el of newDocument.querySelectorAll("head link[rel=stylesheet]")) {
    // Do not preload links that are already on the page.
    if (
      !document.querySelector(
        `[${PERSIST_ATTR}="${el.getAttribute(
          PERSIST_ATTR,
        )}"], link[rel=stylesheet][href="${el.getAttribute("href")}"]`,
      )
    ) {
      const c = document.createElement("link");
      c.setAttribute("rel", "preload");
      c.setAttribute("as", "style");
      c.setAttribute("href", el.getAttribute("href")!);
      links.push(
        new Promise<any>((resolve) => {
          ["load", "error"].forEach((evName) => c.addEventListener(evName, resolve));
          document.head.append(c);
        }),
      );
    }
  }
  return links;
}

// replace head and body of the windows document with contents from newDocument
// if !popstate, update the history entry and scroll position according to toLocation
// if popState is given, this holds the scroll position for history navigation
// if fallback === "animate" then simulate view transitions
async function updateDOM(
  preparationEvent: TransitionBeforePreparationEvent,
  options: Options,
  currentTransition: Transition,
  historyState?: State,
  fallback?: Fallback,
) {
  async function animate(phase: string) {
    function isInfinite(animation: Animation) {
      const effect = animation.effect;
      if (!effect || !(effect instanceof KeyframeEffect) || !effect.target) return false;
      const style = window.getComputedStyle(effect.target, effect.pseudoElement);
      return style.animationIterationCount === "infinite";
    }
    const currentAnimations = document.getAnimations();
    // Trigger view transition animations waiting for data-zfb-transition-fallback
    document.documentElement.setAttribute(OLD_NEW_ATTR, phase);
    const nextAnimations = document.getAnimations();
    const newAnimations = nextAnimations.filter(
      (a) => !currentAnimations.includes(a) && !isInfinite(a),
    );
    // Wait for all new animations to finish (resolved or rejected).
    // Do not reject on canceled ones.
    return Promise.allSettled(newAnimations.map((a) => a.finished));
  }

  const animateFallbackOld = async () => {
    if (
      fallback === "animate" &&
      !currentTransition.transitionSkipped &&
      !preparationEvent.signal.aborted
    ) {
      try {
        await animate("old");
      } catch {
        // animate might reject as a consequence of a call to skipTransition()
        // ignored on purpose
      }
    }
  };

  const pageTitleForBrowserHistory = document.title; // document.title will be overridden by swap()
  // Cancel deferred-hydration callbacks for old-body islands before the swap
  // so rIC / IntersectionObserver fires do not run against orphan elements.
  // Called before doSwap() which dispatches `zfb:before-swap` then mutates the DOM.
  cancelPendingIslands();
  // Unmount mounted islands on the OLD body before the swap so Preact/React
  // trees receive render(null, element) / root.unmount() and their useEffect
  // cleanups fire. Must happen after cancelPendingIslands() and before doSwap()
  // so document.body still points to the old body.
  unmountIslands();
  const swapEvent = await doSwap(
    preparationEvent,
    currentTransition.viewTransition!,
    animateFallbackOld,
  );
  moveToLocation(swapEvent.to, swapEvent.from, options, pageTitleForBrowserHistory, historyState);
  triggerEvent("zfb:after-swap");

  // Resolve the finished promise of the simulation's ViewTransition.
  // For 'animate', wait for the new-page animation to complete first.
  // For other fallback modes (e.g. 'swap'), resolve immediately — no animation needed.
  if (fallback === "animate" && !currentTransition.transitionSkipped && !swapEvent.signal.aborted) {
    animate("new").finally(() => currentTransition.viewTransitionFinished!());
  } else {
    currentTransition.viewTransitionFinished?.();
  }
}

function abortAndRecreateMostRecentNavigation(): Navigation {
  mostRecentNavigation?.controller.abort();
  return (mostRecentNavigation = {
    controller: new AbortController(),
  });
}

async function transition(
  direction: Direction,
  from: URL,
  to: URL,
  options: Options,
  historyState?: State,
  hasUAVisualTransition = false,
) {
  // The most recent navigation always has precedence
  // Yes, there can be several navigation instances as the user can click links
  // while we fetch content or simulate view transitions. Even synchronous creations are possible
  // e.g. by calling navigate() from a transition event.
  // Invariant: all but the most recent navigation are already aborted.

  const currentNavigation = abortAndRecreateMostRecentNavigation();

  // not ours
  if (!transitionEnabledOnThisPage() || location.origin !== to.origin) {
    if (currentNavigation === mostRecentNavigation) mostRecentNavigation = undefined;
    location.href = to.href;
    return;
  }

  const navigationType = historyState
    ? "traverse"
    : options.history === "replace"
      ? "replace"
      : "push";

  if (navigationType !== "traverse") {
    updateScrollPosition({ scrollX, scrollY });
  }
  if (samePage(from, to) && !options.formData) {
    if ((direction !== "back" && to.hash) || (direction === "back" && from.hash)) {
      moveToLocation(to, from, options, document.title, historyState);
      if (currentNavigation === mostRecentNavigation) mostRecentNavigation = undefined;
      return;
    }
  }

  const prepEvent = await doPreparation(
    from,
    to,
    direction,
    navigationType,
    options.sourceElement,
    options.info,
    currentNavigation!.controller.signal,
    options.formData,
    defaultLoader,
  );
  if (prepEvent.defaultPrevented || prepEvent.signal.aborted) {
    if (currentNavigation === mostRecentNavigation) mostRecentNavigation = undefined;
    triggerEvent("zfb:navigation-aborted");
    if (!prepEvent.signal.aborted) {
      // not aborted -> delegate to browser
      location.href = to.href;
    }
    // and / or exit
    return;
  }

  async function defaultLoader(preparationEvent: TransitionBeforePreparationEvent) {
    const href = preparationEvent.to.href;
    const init: RequestInit = { signal: preparationEvent.signal };
    if (preparationEvent.formData) {
      init.method = "POST";
      const form =
        preparationEvent.sourceElement instanceof HTMLFormElement
          ? preparationEvent.sourceElement
          : preparationEvent.sourceElement instanceof HTMLElement &&
              "form" in preparationEvent.sourceElement
            ? (preparationEvent.sourceElement.form as HTMLFormElement)
            : preparationEvent.sourceElement?.closest("form");
      // Form elements without enctype explicitly set default to application/x-www-form-urlencoded.
      // In order to maintain compatibility with Astro 4.x, we need to check the value of enctype
      // on the attributes property rather than accessing .enctype directly. Astro 5.x may
      // introduce defaulting to application/x-www-form-urlencoded as a breaking change, and then
      // we can access .enctype directly.
      //
      // Note: getNamedItem can return null in real life, even if TypeScript doesn't think so, hence
      // the ?.
      init.body =
        form !== undefined &&
        Reflect.get(HTMLFormElement.prototype, "attributes", form).getNamedItem("enctype")
          ?.value === "application/x-www-form-urlencoded"
          ? new URLSearchParams(preparationEvent.formData as any)
          : preparationEvent.formData;
    }
    const response = await fetchHTML(href, init);
    // If there is a problem fetching the new page, just do an MPA navigation to it.
    if (response === null) {
      preparationEvent.preventDefault();
      return;
    }
    // if there was a redirection, show the final URL in the browser's address bar
    if (response.redirected) {
      const redirectedTo = new URL(response.redirected);
      // but do not redirect cross origin
      if (redirectedTo.origin !== preparationEvent.to.origin) {
        preparationEvent.preventDefault();
        return;
      }
      // preserve fragment
      const fragment = preparationEvent.to.hash;
      preparationEvent.to = redirectedTo;
      preparationEvent.to.hash = fragment;
    }

    parser ??= new DOMParser();

    preparationEvent.newDocument = parser.parseFromString(response.html, response.mediaType);
    // The next line might look like a hack,
    // but it is actually necessary as noscript elements
    // and their contents are returned as markup by the parser,
    // see https://developer.mozilla.org/en-US/docs/Web/API/DOMParser/parseFromString
    preparationEvent.newDocument.querySelectorAll("noscript").forEach((el) => el.remove());

    // If ClientRouter is not enabled on the incoming page, do a full page load to it.
    // Unless this was a form submission, in which case we do not want to trigger another mutation.
    if (
      !preparationEvent.newDocument.querySelector('[name="zfb-view-transitions-enabled"]') &&
      !preparationEvent.formData
    ) {
      preparationEvent.preventDefault();
      return;
    }

    const links = preloadStyleLinks(preparationEvent.newDocument);
    links.length && !preparationEvent.signal.aborted && (await Promise.all(links));

    // W3C2: prepareForClientOnlyComponents() goes here. Astro's DEV-only iframe
    // trick that hoists Vite per-component CSS for client:only islands does not
    // apply to zfb (W1B §13.5 — zfb islands inject CSS via the bundle), so the
    // call is intentionally absent rather than stubbed.
  }
  async function abortAndRecreateMostRecentTransition(): Promise<Transition> {
    if (mostRecentTransition) {
      if (mostRecentTransition.viewTransition) {
        try {
          mostRecentTransition.viewTransition.skipTransition();
        } catch {
          // might throw AbortError DOMException. Ignored on purpose.
        }
        try {
          // UpdateCallbackDone might already been settled, i.e. if the previous transition finished updating the DOM.
          // Could not take long, we wait for it to avoid parallel updates
          // (which are very unlikely as long as swap() is not async).
          await mostRecentTransition.viewTransition.updateCallbackDone;
        } catch {
          // There was an error in the update callback of the transition which we cancel.
          // Ignored on purpose
        }
      }
    }
    return (mostRecentTransition = { transitionSkipped: false });
  }

  const currentTransition = await abortAndRecreateMostRecentTransition();

  if (prepEvent.signal.aborted) {
    if (currentNavigation === mostRecentNavigation) mostRecentNavigation = undefined;
    return;
  }

  document.documentElement.setAttribute(DIRECTION_ATTR, prepEvent.direction);
  if (supportsViewTransitions && !hasUAVisualTransition) {
    // This automatically cancels any previous transition
    // We also already took care that the earlier update callback got through
    currentTransition.viewTransition = document.startViewTransition(
      async () => await updateDOM(prepEvent, options, currentTransition, historyState),
    );
  } else {
    // Simulation mode requires a bit more manual work.
    // Also used when PopStateEvent.hasUAVisualTransition indicates the browser already
    // provided a visual transition (e.g. Safari swipe gesture) — in that case, fallback
    // is "swap" to skip animations.
    const updateDone = (async () => {
      // Immediately paused to set up the ViewTransition object for Fallback mode
      await Promise.resolve(); // hop through the micro task queue
      await updateDOM(
        prepEvent,
        options,
        currentTransition,
        historyState,
        hasUAVisualTransition ? "swap" : getFallback(),
      );
      return undefined;
    })();

    // When the updateDone promise is settled,
    // we have run and awaited all swap functions and the after-swap event
    // This qualifies for "updateCallbackDone".
    //
    // For the build in ViewTransition, "ready" settles shortly after "updateCallbackDone",
    // i.e. after all pseudo elements are created and the animation is about to start.
    // In simulation mode the "old" animation starts before swap,
    // the "new" animation starts after swap. That is not really comparable.
    // Thus we go with "very, very shortly after updateCallbackDone" and make both equal.
    //
    // "finished" resolves after all animations are done.

    currentTransition.viewTransition = {
      updateCallbackDone: updateDone, // this is about correct
      ready: updateDone, // good enough
      // Finished promise could have been done better: finished rejects iff updateDone does.
      // Our simulation always resolves, never rejects.
      finished: new Promise((r) => (currentTransition.viewTransitionFinished = r as () => void)), // see end of updateDOM
      skipTransition: () => {
        currentTransition.transitionSkipped = true;
        // This cancels all animations of the simulation
        document.documentElement.removeAttribute(OLD_NEW_ATTR);
      },
      types: new Set<string>(), // empty by default
    };
  }
  // In earlier versions was then'ed on viewTransition.ready which would not execute
  // if the visual part of the transition has errors or was skipped
  currentTransition.viewTransition?.updateCallbackDone.finally(async () => {
    await runScripts();
    // Mount new island markers introduced by the body swap. Fire-and-forget;
    // each island's scheduleHydrate call is async (idle / visible). Called after
    // runScripts() so any new mountIslands() registration from inline scripts in
    // the new page has already run. Called before onPageLoad() per W1B §12.2.
    mountNewIslands();
    onPageLoad();
    announce();
  });
  // finished.ready and finished.finally are the same for the simulation but not
  // necessarily for native view transition, where finished rejects when updateCallbackDone does.
  currentTransition.viewTransition?.finished.finally(() => {
    // exactOptionalPropertyTypes: true forbids assigning `undefined` to an optional
    // `viewTransition?: ViewTransition` slot — `delete` is the equivalent reset.
    delete currentTransition.viewTransition;
    if (currentTransition === mostRecentTransition) mostRecentTransition = undefined;
    if (currentNavigation === mostRecentNavigation) mostRecentNavigation = undefined;
    document.documentElement.removeAttribute(DIRECTION_ATTR);
    document.documentElement.removeAttribute(OLD_NEW_ATTR);
  });
  try {
    // Compatibility:
    // In an earlier version we awaited viewTransition.ready, which includes animation setup.
    // Scripts that depend on the view transition pseudo elements should hook on viewTransition.ready.
    await currentTransition.viewTransition?.updateCallbackDone;
  } catch (e) {
    // This log doesn't make it worse than before, where we got error messages about uncaught exceptions, which can't be caught when the trigger was a click or history traversal.
    // Needs more investigation on root causes if errors still occur sporadically
    const err = e as Error;
    // biome-ignore lint/suspicious/noConsole: allowed
    console.log("[zfb]", err.name, err.message, err.stack);
  }
}

let navigateOnServerWarned = false;

export async function navigate(href: string, options?: Options) {
  if (inBrowser === false) {
    if (!navigateOnServerWarned) {
      // instantiate an error for the stacktrace to show to user.
      const warning = new Error(
        "The view transitions client API was called during a server side render. This may be unintentional as the navigate() function is expected to be called in response to user interactions. Please make sure that your usage is correct.",
      );
      warning.name = "Warning";
      // biome-ignore lint/suspicious/noConsole: allowed
      console.warn(warning);
      navigateOnServerWarned = true;
    }
    return;
  }
  if (!supportsViewTransitions && getFallback() === "none") {
    location.href = new URL(href, location.href).href;
    return;
  }
  await transition("forward", originalLocation, new URL(href, location.href), options ?? {});
}

function onPopState(ev: PopStateEvent) {
  if (!transitionEnabledOnThisPage() && ev.state) {
    // The current page doesn't have View Transitions enabled
    // but the page we navigate to does (because it set the state).
    // Do a full page refresh to reload the client-side router from the new page.
    location.reload();
    return;
  }

  // History entries without state are created by the browser (e.g. for hash links)
  // Our view transition entries always have state.
  // Just ignore stateless entries.
  // The browser will handle navigation fine without our help
  if (ev.state === null) {
    return;
  }
  const state: State = history.state;
  const nextIndex = state.index;
  const direction: Direction = nextIndex > currentHistoryIndex ? "forward" : "back";
  currentHistoryIndex = nextIndex;
  transition(
    direction,
    originalLocation,
    new URL(location.href),
    {},
    state,
    ev.hasUAVisualTransition,
  );
}

const onScrollEnd = () => {
  // NOTE: our "popstate" event handler may call `pushState()` or
  // `replaceState()` and then `scrollTo()`, which will fire "scroll" and
  // "scrollend" events. To avoid redundant work and expensive calls to
  // `replaceState()`, we simply check that the values are different before
  // updating.
  if (history.state && (scrollX !== history.state.scrollX || scrollY !== history.state.scrollY)) {
    updateScrollPosition({ scrollX, scrollY });
  }
};

// initialization
if (inBrowser) {
  if (supportsViewTransitions || getFallback() !== "none") {
    originalLocation = new URL(location.href);
    addEventListener("popstate", onPopState);
    addEventListener("load", onPageLoad);
    // There's not a good way to record scroll position before a history back
    // navigation, so we will record it when the user has stopped scrolling.
    if ("onscrollend" in window) addEventListener("scrollend", onScrollEnd);
    else {
      // Keep track of state between intervals
      let intervalId: number | undefined, lastY: number, lastX: number, lastIndex: State["index"];
      const scrollInterval = () => {
        // Check the index to see if a popstate event was fired
        if (lastIndex !== history.state?.index) {
          clearInterval(intervalId);
          intervalId = undefined;
          return;
        }
        // Check if the user stopped scrolling
        if (lastY === scrollY && lastX === scrollX) {
          // Cancel the interval and update scroll positions
          clearInterval(intervalId);
          intervalId = undefined;
          onScrollEnd();
          return;
        } else {
          ((lastY = scrollY), (lastX = scrollX));
        }
      };
      // We can't know when or how often scroll events fire, so we'll just use them to start intervals
      addEventListener(
        "scroll",
        () => {
          if (intervalId !== undefined) return;
          ((lastIndex = history.state?.index), (lastY = scrollY), (lastX = scrollX));
          intervalId = window.setInterval(scrollInterval, 50);
        },
        { passive: true },
      );
    }
  }
  for (const script of document.getElementsByTagName("script")) {
    detectScriptExecuted(script);
    script.dataset["zfbExec"] = "";
  }
}

// ---- W3C3: click + form intercept, public idempotent init() ----

// Returns true when the modifier-key combo or mouse button means "open in new tab / download".
// Matches Astro's `leavesWindow` helper in ClientRouter.astro.
const leavesWindow = (ev: MouseEvent): boolean =>
  (ev.button !== undefined && ev.button !== 0) || // non-left-click
  ev.metaKey || // new tab (Mac)
  ev.ctrlKey || // new tab (Windows/Linux)
  ev.altKey || // download
  ev.shiftKey; // new window

// Track the last clicked element that will leave the window so form submit can check it.
let lastClickedElementLeavingWindow: EventTarget | null = null;

function handleClick(ev: MouseEvent): void {
  let link: EventTarget | null = ev.target;

  // Record whether this click will leave the window (used by form submit handler).
  lastClickedElementLeavingWindow = leavesWindow(ev) ? link : null;

  // Shadow DOM: prefer composedPath target over ev.target.
  if (ev.composed) {
    link = ev.composedPath()[0] ?? link;
  }

  // Walk up to the nearest <a>, <area>, or <svg:a>.
  if (link instanceof Element) {
    link = link.closest("a, area");
  }

  if (
    !(link instanceof HTMLAnchorElement) &&
    !(link instanceof SVGAElement) &&
    !(link instanceof HTMLAreaElement)
  ) {
    return;
  }

  const linkEl = link as HTMLAnchorElement | SVGAElement | HTMLAreaElement;
  const linkTarget =
    linkEl instanceof HTMLElement ? linkEl.target : (linkEl as SVGAElement).target.baseVal;
  const href = linkEl instanceof HTMLElement ? linkEl.href : (linkEl as SVGAElement).href.baseVal;

  if (!href) return;

  const origin = new URL(href, location.href).origin;

  if (
    // data-zfb-reload: caller wants a full browser reload, not a SPA transition.
    (linkEl as HTMLElement).dataset["zfbReload"] !== undefined ||
    // download attribute: let browser handle download.
    linkEl.hasAttribute("download") ||
    // Non-self target opens in a new context — skip.
    (linkTarget && linkTarget !== "_self") ||
    // Cross-origin: not ours to handle.
    origin !== location.origin ||
    // Modifier key / non-left-click combo: user wants new tab / window / download.
    lastClickedElementLeavingWindow !== null ||
    // Another handler already handled this event.
    ev.defaultPrevented
  ) {
    return;
  }

  ev.preventDefault();
  navigate(href, {
    // data-zfb-history="replace" opts a link into replaceState instead of pushState.
    history: (linkEl as HTMLElement).dataset["zfbHistory"] === "replace" ? "replace" : "auto",
    sourceElement: linkEl,
  });
}

function handleSubmit(ev: SubmitEvent): void {
  const el = ev.target as HTMLElement;
  const submitter = ev.submitter as HTMLElement | null;

  // If the submit was triggered by a modifier-key click, treat as normal browser submit.
  const clickedWithKeys = submitter !== null && submitter === lastClickedElementLeavingWindow;
  lastClickedElementLeavingWindow = null;

  if (
    el.tagName !== "FORM" ||
    ev.defaultPrevented ||
    el.dataset["zfbReload"] !== undefined ||
    clickedWithKeys
  ) {
    return;
  }

  const form = el as HTMLFormElement;
  const formData = new FormData(form, submitter ?? undefined);

  // form.action / form.method can be shadowed by <input name="action"> / <input name="method">,
  // so fall back to getAttribute() when the property is not a string. (Astro's comment.)
  const formAction = typeof form.action === "string" ? form.action : form.getAttribute("action");
  const formMethod = typeof form.method === "string" ? form.method : form.getAttribute("method");

  // Resolve action: submitter formaction attr overrides form action, fallback to current path.
  let action = submitter?.getAttribute("formaction") ?? formAction ?? location.pathname;
  // Resolve method: submitter formmethod attr overrides form method, fallback to "get".
  const method = submitter?.getAttribute("formmethod") ?? formMethod ?? "get";

  // The "dialog" method is a special keyword used within <dialog> elements —
  // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-method
  if (method === "dialog" || location.origin !== new URL(action, location.href).origin) {
    // No SPA transition in these cases — let browser handle it.
    return;
  }

  const options: import("./types.js").Options = { sourceElement: submitter ?? form };
  if (method === "get") {
    const params = new URLSearchParams(formData as any);
    const url = new URL(action, location.href);
    url.search = params.toString();
    action = url.toString();
  } else {
    options.formData = formData;
  }

  ev.preventDefault();
  navigate(action, options);
}

export interface InitOptions {
  /** Reserved for forward-compat with Astro's prefetch integration. Ignored in v1. */
  prefetchAll?: boolean;
}

// Guard flag — ensures click + submit listeners are registered only once even if
// init() is called multiple times (e.g. two <ClientRouter> mounts on the same page).
let initialized = false;

/**
 * Wire up the client-router's click and form-submit intercepts.
 * Safe to call multiple times — subsequent calls are no-ops (idempotent).
 *
 * @param _options - Forward-compat hook matching Astro's init() signature. Ignored in v1.
 */
export function init(_options?: InitOptions): void {
  if (initialized) return;
  initialized = true;

  if (!inBrowser) return;
  if (!supportsViewTransitions && getFallback() === "none") return;

  document.addEventListener("click", handleClick);
  document.addEventListener("submit", handleSubmit as EventListener);

  // Prefetch hook intentionally omitted from v1 — see https://github.com/zudolab/zudo-doc/issues/1527
  // (Followup tracker for porting Astro prefetch module).
}
