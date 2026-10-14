// Stub for `node:buffer`. See node_stub.js header.
//
// Note: `Buffer` is sometimes assumed by Node-targeted libraries to
// exist on `globalThis` too. We deliberately do NOT install a global
// `Buffer` — code that does want a buffer in the SSG runtime should
// use `Uint8Array` (web platform) instead. If a real bundle surfaces
// a regression, fix it at the source rather than expanding this stub.
import { makeNodeStubProxy } from "ext:zfb_node_stubs/node_stub.js";

const proxy = makeNodeStubProxy();

export const Buffer = proxy;
export const constants = proxy;
export const kMaxLength = proxy;
export const kStringMaxLength = proxy;

export default proxy;
