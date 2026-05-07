// `@takazudo/zfb-runtime` — `<ViewTransitions />` component (typed no-op).
//
// Originally mirrored Astro's `<ViewTransitions />` integration by injecting
// (a) a `<meta name="view-transition" content="same-origin">` opt-in tag and
// (b) an inline client-router IIFE that intercepted same-origin `<a>` clicks
// and ran navigations through `document.startViewTransition`.
//
// Both pieces have since been deleted. The browser story moved on:
//
//   - The meta-tag opt-in was a temporary experiment during Chromium's
//     cross-document VT spec development. It has been superseded by the
//     `@view-transition { navigation: auto; }` CSS at-rule (per MDN and
//     https://developer.chrome.com/docs/web-platform/view-transitions/cross-document)
//     and is no longer honoured by browsers that ship cross-document VT.
//   - The click-intercept IIFE was actively harmful: it called
//     `event.preventDefault()` and ran `window.location.href = url` inside
//     `document.startViewTransition`. Chromium treats that script reload as
//     EXCLUDED from the `auto` navigation type, so no `::view-transition-*`
//     pseudo-elements ever materialise. The "router" prevented the very
//     feature it claimed to enable.
//
// Cross-document View Transitions are now opted in via the
// `@view-transition { navigation: auto; }` CSS at-rule on both the source
// and destination documents. This component is a typed no-op kept so that
// existing `<ViewTransitions />` mounts compile unchanged.

/**
 * Public element shape — kept structural so consumers don't infer through
 * either framework's internal VNode type. Matches the shape `Island`
 * returns from `@takazudo/zfb`.
 */
export type ViewTransitionsElement = {
  readonly type: string;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * `<ViewTransitions />` — DEPRECATED: this component is now a typed
 * no-op. Cross-document View Transitions are opted in via the
 * `@view-transition { navigation: auto; }` CSS at-rule, NOT via this
 * component. Add the at-rule to your top-level stylesheet (outside
 * any `@layer` block — see https://developer.mozilla.org/en-US/docs/Web/CSS/@view-transition).
 *
 * The export is kept so consumers' existing `<ViewTransitions />`
 * mounts compile unchanged.
 *
 * The previous implementation injected an inline router IIFE that
 * called `event.preventDefault()` and `document.startViewTransition`
 * around `window.location.href = url`. That pattern is INCOMPATIBLE
 * with cross-document VT: Chromium treats the script reload as
 * excluded from `auto`, so no `::view-transition-*` pseudo-elements
 * ever materialize. See
 * https://developer.chrome.com/docs/web-platform/view-transitions/cross-document.
 *
 * @deprecated The runtime injection is removed. Use the
 * `@view-transition` CSS at-rule on both source and destination
 * documents.
 */
export function ViewTransitions(): readonly ViewTransitionsElement[] {
  return [];
}
