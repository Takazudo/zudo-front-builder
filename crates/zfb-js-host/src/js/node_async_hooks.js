// Stub for `node:async_hooks`. See node_stub.js header.
//
// Consumer bundles that import `@takazudo/zfb-adapter-cloudflare` or other
// packages that use `AsyncLocalStorage` / `AsyncResource` will include a
// top-level `import { AsyncLocalStorage } from "node:async_hooks"`. The SSG
// embedded V8 host must resolve this import at module-load time so the bundle
// evaluates at all — but actual use of these APIs during SSG paths() evaluation
// is never needed (the Cloudflare adapter is only invoked at request time in
// the deployed Worker, not during static-site generation).
//
// All exports are throwing proxies. Code paths that really call
// `new AsyncLocalStorage()` during SSG would surface the error with a clear
// "node:* is not available under the SSG runtime" message.
import { makeNodeStubProxy } from "ext:zfb_node_stubs/node_stub.js";

const proxy = makeNodeStubProxy();

export const AsyncLocalStorage = proxy;
export const AsyncResource = proxy;
export const asyncWrapProviders = proxy;
export const createHook = proxy;
export const executionAsyncId = proxy;
export const executionAsyncResource = proxy;
export const triggerAsyncId = proxy;

export default proxy;
