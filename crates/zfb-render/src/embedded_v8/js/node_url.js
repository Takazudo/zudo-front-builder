// Stub for `node:url`. See node_stub.js header.
//
// Note: `node:url` is distinct from the global `URL` constructor —
// `node:url` is Node's deprecated string-based URL utilities.
// The bundle should be using `globalThis.URL` from `deno_web` for
// real URL parsing; this stub is only for code paths that
// happened to import the Node namespace.
import { makeNodeStubProxy } from "ext:zfb_node_stubs/node_stub.js";

const proxy = makeNodeStubProxy();

export const parse = proxy;
export const format = proxy;
export const resolve = proxy;
export const fileURLToPath = proxy;
export const pathToFileURL = proxy;
// Re-export the throwing proxy as `URL`/`URLSearchParams` too — user
// code that imports those by name from `node:url` (rather than the
// global) gets the same throw-on-use behaviour.
export const URL = proxy;
export const URLSearchParams = proxy;

export default proxy;
