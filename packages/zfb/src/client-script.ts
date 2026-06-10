/**
 * SSR helper for referencing a client-script asset URL in a page or layout.
 *
 * ## Usage
 *
 * ```tsx
 * import { clientScript } from "@takazudo/zfb";
 *
 * export default function MyPage() {
 *   return (
 *     <html>
 *       <head>
 *         <script type="module" src={clientScript("search-widget")} />
 *       </head>
 *       <body>…</body>
 *     </html>
 *   );
 * }
 * ```
 *
 * ## How it works
 *
 * `clientScript(name)` returns the **stable URL** for the named client-script
 * entry: `${base}/assets/client/<name>.js`. The production build pipeline
 * (`ProductionAssetPipeline`) rewrites every occurrence of this stable URL in
 * rendered HTML to the content-hashed equivalent
 * (`/assets/client/<name>-<hash>.js`), so the hash never needs to be known at
 * SSR time.
 *
 * The `base` prefix is read from `globalThis.__zfb?.base` (emitted by the
 * bundler when at least one `*.client.*` file exists in the project). For
 * root-mounted sites (`base` is absent or `"/"`), the prefix is the empty
 * string and `clientScript("search-widget")` returns
 * `"/assets/client/search-widget.js"`. For a sub-path deploy
 * (`base="/foo/"`), it returns `"/foo/assets/client/search-widget.js"`.
 *
 * ## SSR-only note (v1)
 *
 * This is an SSR-time helper. Calling it in browser-executed code works but
 * the base prefix (`globalThis.__zfb.base`) is currently not shipped to the
 * browser — `clientScript()` will return the unprefixed stable URL
 * (`/assets/client/<name>.js`) in that context. For the common use-case of
 * rendering a `<script src>` tag at SSR time this is not a problem.
 */

/** Stable public-URL prefix for client-script entries, matching the Rust constant. */
const CLIENT_SCRIPTS_URL_PREFIX = "/assets/client/";

/**
 * Returns the base-prefixed stable public URL for the named client-script entry.
 *
 * `name` is the entry name (file stem minus `.client`, e.g. `"search-widget"`
 * for `search-widget.client.ts`).
 *
 * The returned URL is `${base}/assets/client/<name>.js`. The production
 * pipeline rewrites it to the hashed URL; dev serves the stable URL directly.
 */
export function clientScript(name: string): string {
  // `globalThis.__zfb?.base` is set to the resolved base prefix by the
  // bundler when the project has at least one `*.client.*` entry (#978).
  // For root-mounted or no-base builds the value is `""`, yielding
  // `/assets/client/<name>.js` as expected. The `?? ""` fallback handles
  // the edge case where __zfb.base was not emitted (zero-script project,
  // or a browser context where the slot was never populated).
  const base = (globalThis as Record<string, unknown>).__zfb as { base?: string } | undefined;
  const prefix = base?.base ?? "";
  return `${prefix}${CLIENT_SCRIPTS_URL_PREFIX}${name}.js`;
}
