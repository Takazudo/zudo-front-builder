// Stub for `node:fs`. See node_stub.js header.
import { makeNodeStubProxy } from "ext:zfb_node_stubs/node_stub.js";

const proxy = makeNodeStubProxy();

// Common named exports user bundles import. Each is the same throwing
// proxy — we don't need per-name fidelity because nothing should
// actually run.
export const readFile = proxy;
export const readFileSync = proxy;
export const writeFile = proxy;
export const writeFileSync = proxy;
export const stat = proxy;
export const statSync = proxy;
export const promises = proxy;
export const existsSync = proxy;
export const createReadStream = proxy;
export const createWriteStream = proxy;

export default proxy;
