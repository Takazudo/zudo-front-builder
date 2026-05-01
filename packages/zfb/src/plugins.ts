// `zfb/plugins` — TypeScript helper for the zfb plugin lifecycle.
//
// A plugin is a JS module whose default export is a [`ZfbPlugin`] object.
// `zfb.config.ts` references plugins by `name` (npm bare specifier or a
// `./`-relative path); the zfb config loader resolves each `name` to an
// absolute module specifier and the Rust-side plugin host loads the
// module via dynamic `import()` and dispatches the lifecycle hooks.
//
// Sub 3 / issue #108 — initial drop. Three optional hooks: `preBuild`,
// `postBuild`, `devMiddleware`. None of the hooks see real Node IPC
// objects across the boundary; everything is JSON-friendly.
//
// ## Inline functions are NOT supported
//
// `PluginConfig` (in `./config.ts`) carries only data. A user cannot
// inline a function in `zfb.config.ts` — the config goes through a
// JSON round-trip and any function value would be silently dropped.
// Plugins must live in their own module (npm package or local file)
// and be referenced by `name`.

/**
 * Logger handed to every plugin hook. The Rust side wraps `tracing` so
 * the same lines show up alongside the rest of the build's structured
 * logs. Hooks should prefer this over `console.log`.
 */
export type ZfbPluginLogger = {
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
};

/**
 * Context passed to `preBuild` and `postBuild`. `outDir` is the
 * resolved absolute path of the configured `outDir` (default
 * `<projectRoot>/dist`). `projectRoot` is the directory containing
 * `zfb.config.ts`.
 */
export type ZfbBuildHookContext = {
  /** Project root — the directory containing `zfb.config.ts`. */
  projectRoot: string;
  /** Resolved absolute path of the build output directory. */
  outDir: string;
  /** The full loaded `ZfbConfig` (data-only view). */
  config: import("./config.js").ZfbConfig;
  /** Plugin-specific options block, copied verbatim from the matching `PluginConfig.options`. */
  options: Record<string, unknown>;
  /** Logger that wraps the Rust-side `tracing` subscriber. */
  logger: ZfbPluginLogger;
};

/**
 * A request handed to a `devMiddleware` handler. Subset of the Node
 * `http.IncomingMessage` surface intentionally — the dev server is
 * Rust-side `axum`, not Node, so we expose only what survives a JSON
 * envelope hop.
 */
export type ZfbDevMiddlewareRequest = {
  method: string;
  url: string;
  /** Lower-cased header names → first value. */
  headers: Record<string, string>;
  /** Raw request body; absent for GET/HEAD. UTF-8 only — binary is out of scope for v1 dev plugins. */
  body?: string;
};

/**
 * Response returned by a `devMiddleware` handler. All fields optional
 * except `status`. `body` may be a string (UTF-8) or a base64-encoded
 * binary payload (set `bodyEncoding` to `"base64"` in that case).
 */
export type ZfbDevMiddlewareResponse = {
  status: number;
  headers?: Record<string, string>;
  body?: string;
  bodyEncoding?: "utf8" | "base64";
};

/**
 * Handler signature for a `devMiddleware` registration. The `next` callback
 * is reserved for future composition; v1 plugins should produce a response
 * directly. Returning `undefined` from the handler signals "I did not handle
 * this request" — the dev server then falls through to its built-in routes
 * (the page cache, /__zfb/livereload.js, etc.).
 */
export type ZfbDevMiddlewareHandler = (
  req: ZfbDevMiddlewareRequest,
) => Promise<ZfbDevMiddlewareResponse | undefined> | ZfbDevMiddlewareResponse | undefined;

/**
 * Context passed to `devMiddleware`. The `register` callback installs
 * one handler per URL path prefix. `path` is matched as an exact prefix
 * — a registration on `/doc-history` matches `/doc-history` and
 * `/doc-history/foo`, but NOT `/doc-historyx`.
 */
export type ZfbDevMiddlewareContext = {
  projectRoot: string;
  config: import("./config.js").ZfbConfig;
  options: Record<string, unknown>;
  logger: ZfbPluginLogger;
  /** Register an HTTP handler at `path`. Calling twice on the same path overwrites. */
  register(path: string, handler: ZfbDevMiddlewareHandler): void;
};

/**
 * The plugin-module shape. `name` is informational (the resolved module
 * specifier wins for identification on the Rust side) and helps the
 * plugin self-identify in logs.
 */
export type ZfbPlugin = {
  /** Plugin display name; surfaces in error / log lines. */
  name: string;
  preBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  postBuild?(ctx: ZfbBuildHookContext): Promise<void> | void;
  devMiddleware?(ctx: ZfbDevMiddlewareContext): Promise<void> | void;
};

/**
 * Identity helper that types the supplied object as a [`ZfbPlugin`].
 * Use as the default export of a plugin module so editors surface
 * field-level types and typos surface at compile time.
 *
 * ```ts
 * import { definePlugin } from "@takazudo/zfb/plugins";
 *
 * export default definePlugin({
 *   name: "my-plugin",
 *   async preBuild({ outDir, logger }) {
 *     logger.info(`generating index into ${outDir}`);
 *   },
 * });
 * ```
 */
export function definePlugin(plugin: ZfbPlugin): ZfbPlugin {
  return plugin;
}
