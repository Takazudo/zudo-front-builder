// Shared sentinel implementation for the `node:*` stub modules.
//
// User bundles may include top-level
// `import "node:fs"` for code paths that only fire under Workers /
// production SSR. We resolve the import at module-load time so the
// bundle evaluates, but every property access / call throws so any
// real attempt to use Node APIs from the SSG runtime fails with a
// recognisable error message.
//
// This file is referenced by the per-specifier shims below via
// `import { ... } from "ext:zfb_node_stubs/node_stub.js"`.

const ERROR_MSG = "node:* is not available under the SSG runtime";

// `__zfbThrowNodeStub` is the named sentinel — search the codebase
// for it before renaming. It throws a plain `Error` so the V8 stack
// trace surfaces in the host's `RenderError::Runtime(...)` payload.
function __zfbThrowNodeStub() {
  throw new Error(ERROR_MSG);
}

// Build a Proxy whose every property access / function call throws
// the standard error. This is what each `node:*` module's `default`
// export resolves to, AND what every named-export read returns.
//
// The trap shape covers:
//   import fs from "node:fs";          fs.readFileSync(...);   // throws on .readFileSync get
//   import { readFile } from "node:fs/promises"; await readFile(...);  // throws on apply
//   import * as path from "node:path"; path.join(...);         // throws on .join get
function makeNodeStubProxy() {
  const target = function () {
    __zfbThrowNodeStub();
  };
  return new Proxy(target, {
    get(_obj, prop) {
      // Allow a couple of standard introspection properties so a
      // *typeof* check at the import site doesn't crash before
      // user code has a chance to fall back. These mirror what
      // a regular function exposes.
      if (
        prop === Symbol.toPrimitive ||
        prop === Symbol.toStringTag ||
        prop === "then" /* avoid await-on-import lockup */ ||
        prop === "constructor"
      ) {
        return undefined;
      }
      __zfbThrowNodeStub();
    },
    apply() {
      __zfbThrowNodeStub();
    },
    construct() {
      __zfbThrowNodeStub();
    },
    has() {
      // `"x" in stub` returns false; lets feature-detection paths
      // bail cleanly instead of throwing on the probe.
      return false;
    },
  });
}

export { makeNodeStubProxy, __zfbThrowNodeStub };
