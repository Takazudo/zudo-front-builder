// Stub for `node:fs/promises`. See node_stub.js header.
import { makeNodeStubProxy } from "ext:zfb_node_stubs/node_stub.js";

const proxy = makeNodeStubProxy();

export const readFile = proxy;
export const writeFile = proxy;
export const stat = proxy;
export const access = proxy;
export const mkdir = proxy;
export const rm = proxy;
export const readdir = proxy;
export const unlink = proxy;
export const open = proxy;

export default proxy;
