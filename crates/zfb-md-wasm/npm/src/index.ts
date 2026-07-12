import type { CompileResult, RenderHtmlResult, ZfbMdWasmOptions } from "./types.js";

// The wasm-bindgen-generated glue lives at ./wasm/zfb_md_wasm.{js,d.ts} plus
// the ./wasm/zfb_md_wasm_bg.wasm binary, produced by `pnpm build`
// (scripts/build.mjs) -- not checked in, see .gitignore. `pnpm build` must
// run before this module is imported (or before `tsc` can typecheck it).
type WasmGlueModule = typeof import("./wasm/zfb_md_wasm.js");
type WasmRawExports = import("./wasm/zfb_md_wasm.js").InitOutput;

const GLUE_URL = new URL("./wasm/zfb_md_wasm.js", import.meta.url);
const WASM_URL = new URL("./wasm/zfb_md_wasm_bg.wasm", import.meta.url);

function isNode(): boolean {
  return typeof process !== "undefined" && !!process.versions?.node;
}

// Node's built-in `fetch` does not support `file:` URLs, so the wasm bytes
// must be read from disk directly there; browsers (and other fetch-capable
// hosts, e.g. a bundler-served dev server) go through `fetch` against the
// module-relative URL, which is the standard wasm-bindgen `--target web`
// browser consumption path.
async function loadWasmBytes(): Promise<ArrayBuffer> {
  if (isNode()) {
    const [{ readFile }, { fileURLToPath }] = await Promise.all([
      import("node:fs/promises"),
      import("node:url"),
    ]);
    const buf = await readFile(fileURLToPath(WASM_URL));
    return buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength) as ArrayBuffer;
  }
  const res = await fetch(WASM_URL);
  if (!res.ok) {
    throw new Error(`zfb-md-wasm: failed to fetch wasm binary: ${res.status} ${res.statusText}`);
  }
  return res.arrayBuffer();
}

let compiledModulePromise: Promise<WebAssembly.Module> | undefined;

function getCompiledModule(): Promise<WebAssembly.Module> {
  compiledModulePromise ??= loadWasmBytes().then((bytes) => WebAssembly.compile(bytes));
  return compiledModulePromise;
}

let generation = 0;

interface Instance {
  glue: WasmGlueModule;
  raw: WasmRawExports;
}

/**
 * Instantiates a fresh copy of the wasm-bindgen glue module.
 *
 * wasm-bindgen's `--target web` output guards `init`/`initSync` with an
 * "already initialized" early return keyed on a module-scope singleton, so
 * recovering from a trapped (poisoned) instance requires a genuinely fresh
 * ES module record -- re-calling `initSync` on the existing module record is
 * a no-op. Re-importing the same specifier with a distinct query string
 * yields a new module record per the ECMAScript module spec (Node and
 * browsers agree on this); the underlying `WebAssembly.Module` (compiled
 * code) is cached and reused via `getCompiledModule`, so this only re-runs
 * cheap instantiation, never a recompile.
 */
async function freshInstance(): Promise<Instance> {
  generation += 1;
  const [module, glue] = await Promise.all([
    getCompiledModule(),
    import(
      /* @vite-ignore */ `${GLUE_URL.href}?zfbMdWasmGen=${generation}`
    ) as Promise<WasmGlueModule>,
  ]);
  const raw = glue.initSync({ module });
  return { glue, raw };
}

let instancePromise: Promise<Instance> | undefined;

function getInstance(): Promise<Instance> {
  instancePromise ??= freshInstance();
  return instancePromise;
}

/**
 * Thrown when a wasm call traps (a Rust panic, or another internal fault
 * that lowers to a wasm trap). This is always a bug, never an expected
 * input-validation failure -- expected failures (parse errors, malformed
 * options JSON, unknown themes, ...) come back as structured
 * `Diagnostic[]` in a normal `CompileResult`/`RenderHtmlResult`, never as a
 * thrown error. See this package's README "Error / trap / re-init
 * contract" section.
 *
 * By the time this is thrown, the poisoned instance has already been
 * dropped and a replacement is being instantiated in the background (from
 * the cached compiled module, so no recompile) -- the next `compile` /
 * `renderHtml` / `version` call transparently uses the fresh instance.
 */
export class ZfbMdWasmTrapError extends Error {
  constructor(cause: unknown) {
    super(
      "zfb-md-wasm: the wasm instance trapped (a Rust panic or internal fault) and has been " +
        "automatically re-instantiated. This is always a bug in zfb-md-wasm -- please report it " +
        "with the input that triggered it.",
    );
    this.name = "ZfbMdWasmTrapError";
    this.cause = cause;
  }
}

function isTrap(err: unknown): boolean {
  return typeof WebAssembly !== "undefined" && err instanceof WebAssembly.RuntimeError;
}

async function callWasm<T>(fn: (instance: Instance) => T): Promise<T> {
  const instance = await getInstance();
  try {
    return fn(instance);
  } catch (err) {
    if (!isTrap(err)) {
      throw err;
    }
    // Drop the poisoned instance and eagerly start re-instantiating it (from
    // the cached compiled module) so the next call does not pay the
    // instantiation cost synchronously.
    instancePromise = freshInstance();
    throw new ZfbMdWasmTrapError(err);
  }
}

/**
 * Eagerly loads and instantiates the wasm module. Optional -- `compile` /
 * `renderHtml` / `version` all call this implicitly on first use -- but
 * useful to front-load the one-time fetch/compile cost, e.g. on app
 * startup before the first user-triggered call.
 */
export async function init(): Promise<void> {
  await getInstance();
}

/**
 * Compile MDX source into ES-module JavaScript. Mirrors the crate's
 * `compile(source, options_json) -> string` export, with JSON
 * marshaling handled for you.
 */
export async function compile(
  source: string,
  options: ZfbMdWasmOptions = {},
): Promise<CompileResult> {
  const optionsJson = JSON.stringify(options);
  const json = await callWasm(({ glue }) => glue.compile(source, optionsJson));
  return JSON.parse(json) as CompileResult;
}

/**
 * Render markdown source to an HTML fragment (no SWC at runtime). Mirrors
 * the crate's `renderHtml(source, options_json) -> string` export, with
 * JSON marshaling handled for you.
 */
export async function renderHtml(
  source: string,
  options: ZfbMdWasmOptions = {},
): Promise<RenderHtmlResult> {
  const optionsJson = JSON.stringify(options);
  const json = await callWasm(({ glue }) => glue.renderHtml(source, optionsJson));
  return JSON.parse(json) as RenderHtmlResult;
}

/** This crate's version (`CARGO_PKG_VERSION`), for host-side compatibility checks. */
export async function version(): Promise<string> {
  return callWasm(({ glue }) => glue.version());
}

/**
 * @internal Test-only (zfb#1577's trap/re-init test). Forces a genuine wasm
 * trap on the currently-active instance by invoking its raw `compile` export
 * with a garbage return pointer, so the result-doubleword `i32.store` lands
 * outside the instance's linear memory -- a `WebAssembly.RuntimeError:
 * memory access out of bounds`. From the JS host's point of view this is
 * the same exception class a real Rust panic->unreachable trap produces
 * (both are `instanceof WebAssembly.RuntimeError`), which is what
 * `callWasm`'s catch clause keys on -- so this exercises the real
 * catch-and-reinstantiate path without needing to coax an actual Rust panic
 * out of a crate that is deliberately designed to never panic on structured
 * input. Not exported from package.json's "exports" map (only reachable via
 * a direct relative import into `src/`/`dist/`, which is how the test file
 * uses it) and not part of the supported public API -- the raw ABI details
 * it pokes at (argument order, retptr convention) are wasm-bindgen internals
 * that can change on a wasm-bindgen version bump.
 */
export async function __forceTrapForTests(): Promise<void> {
  await callWasm(({ raw }) => {
    raw.compile(0xfffffff0, 0, 0, 0, 0);
  });
}

export type {
  CompileResult,
  RenderHtmlResult,
  Diagnostic,
  DiagnosticSource,
  ZfbMdWasmOptions,
  PipelineOptions,
  GfmOptions,
  MarkdownFeaturesConfig,
  JsxRuntime,
} from "./types.js";
