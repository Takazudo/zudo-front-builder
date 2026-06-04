// `@takazudo/zfb-adapter-cloudflare/build` — Node-only build helpers.
//
// This sub-entry is intentionally **not** imported by the Workers-runtime
// entry (`./`). Code in this module may freely use Node built-ins
// (`node:fs`, `node:path`, …) because it only ever runs in a Node 22+
// build environment, never inside a Cloudflare Worker isolate.
//
// The `./` entry (`src/index.ts`) exports only Workers-runtime-safe code
// (AsyncLocalStorage helpers) that can be bundled into the worker.

// @ts-expect-error worker-wrapper.mjs has no declaration file; the export
// shape is narrowed explicitly below.
import { WORKER_WRAPPER_SOURCE as _wrapper } from "./worker-wrapper.mjs";
// @ts-expect-error emit-worker.mjs has no declaration file; the export
// shape is narrowed via EmitWorkerInput/EmitWorkerOutput below.
import { emitWorker as _emitWorker } from "./emit-worker.mjs";

/**
 * The wrapper source written to `_worker.js`. Imported from the single
 * canonical `.mjs` file so `src/build.ts` and `bin/cli.mjs` always stay
 * in sync without any duplication.
 */
export const WORKER_WRAPPER_SOURCE: string = _wrapper as string;

/**
 * Inputs to [`emitWorker`].
 */
export interface EmitWorkerInput {
  /**
   * Absolute path to the input ESM bundle produced by `zfb-build`. The
   * bundle must export a Workers-shaped `default { fetch: (request) =>
   * Promise<Response> }` (this is the contract `zfb_build::bundler`
   * pins). The file is copied verbatim next to the emitted wrapper so
   * relative imports inside it keep resolving.
   */
  readonly inputBundlePath: string;
  /**
   * Absolute path to the output directory. The emitter creates it if
   * missing and writes `_worker.js` plus `_zfb_inner.mjs` (the copied
   * input bundle) into it.
   */
  readonly outdir: string;
}

/**
 * Output paths the emitter produced. Returned for callers that want to
 * log them (the Rust orchestrator surfaces them in build output).
 */
export interface EmitWorkerOutput {
  readonly workerPath: string;
  readonly innerBundlePath: string;
}

/**
 * Emit a Cloudflare Pages `_worker.js` that wraps the zfb input bundle.
 *
 * Output shape (two files in `outdir`):
 *
 *   _worker.js       — entry imported by Cloudflare Pages advanced mode
 *   _zfb_inner.mjs   — the input bundle, copied verbatim
 *
 * The wrapper imports the inner bundle via the relative path
 * `./_zfb_inner.mjs`. Workerd's Module loader resolves relative ESM
 * imports inside an advanced-mode `_worker.js` directory, so this layout
 * works without re-bundling.
 *
 * Why two files instead of one: re-bundling here would require a second
 * esbuild pass and would force the adapter to ship its own esbuild
 * binary slot. The two-file layout keeps the adapter dependency-free at
 * runtime — it is just `node:fs` glue.
 */
export async function emitWorker(input: EmitWorkerInput): Promise<EmitWorkerOutput> {
  return _emitWorker({
    inputBundlePath: input.inputBundlePath,
    outdir: input.outdir,
    workerWrapperSource: WORKER_WRAPPER_SOURCE,
  }) as Promise<EmitWorkerOutput>;
}
