// `@takazudo/zfb-runtime` — framework adapter contract.
//
// The runtime is JSX-runtime-agnostic: it never imports preact or react
// directly. Instead, the caller of [`createPageRouter`] passes a
// [`FrameworkAdapter`] that pins how a JSX vnode (the value returned by
// a page module's `default` export) becomes a string of HTML.
//
// This mirrors ADR-002's framework-adapter pattern on the Rust side — the
// runtime owns the page-module evaluation lifecycle, the adapter owns the
// JSX-runtime-specific render call. Wave 2 (T6) wires miniflare + an
// adapter built on `preact-render-to-string` (or `react-dom/server`).

/**
 * Pluggable per-framework SSR contract.
 *
 * `renderToString` takes whatever shape the framework's JSX pragma
 * produces (Preact's VNode, React's element, etc.) and returns a string
 * of HTML. The runtime treats both the input and the return value as
 * opaque — it neither inspects the vnode nor escapes the output.
 *
 * `hydrate` is reserved for future client-side use (e.g. island
 * hydration boot). Today the runtime does not call it; it exists in the
 * type so future SSR-with-hydration deliverables can extend the contract
 * without breaking imports.
 */
export interface FrameworkAdapter {
  /**
   * Render a JSX vnode to an HTML string. Must be synchronous; SSR
   * happens inside a Hono request handler that resolves the response
   * body in one step.
   */
  renderToString: (vnode: unknown) => string;
  /**
   * Optional client-side hydrate hook. Reserved for follow-up work — the
   * page router does not invoke this today.
   */
  hydrate?: (...args: unknown[]) => unknown;
}
