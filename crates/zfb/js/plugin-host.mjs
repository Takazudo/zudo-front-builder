// AUTO-LOADED by zfb-build's plugin runner (Phase B Sub 3 / issue #108,
// extended for the new `setup` hook in epic #253 / sub-issue #255).
// Do not edit unless you also update the Rust caller in
// `crates/zfb-build/src/plugin_runner.rs`.
//
// One long-lived Node subprocess that owns the user's plugin modules
// and dispatches lifecycle hooks. Communicates with the Rust parent
// over stdio with newline-delimited JSON messages.
//
// Protocol (one JSON object per stdin line):
//
//   { "id": <number>, "kind": "init", "plugins": [
//        { "module": "<file-url>", "name": "<display>", "options": {...} },
//        ...
//      ] }
//
//   { "id": <number>, "kind": "setup", "ctx": { "projectRoot",
//        "command": "build" | "dev", "config" } }
//        -- calls each plugin's `setup(ctx)`. ctx exposes
//           addAlias, addVirtualModule, injectRoute, addClientEntry. The host
//           accumulates raw registrations per plugin and replies
//           with `{ outputs: [{ plugin, registrations: [...] }] }`
//           so Rust can run conflict detection against canonical types.
//
//   { "id": <number>, "kind": "preBuild", "ctx": { "projectRoot",
//        "outDir", "config" } }   -- runs preBuild on every plugin in order
//
//   { "id": <number>, "kind": "postBuild", "ctx": { ... } }
//
//   { "id": <number>, "kind": "devRegister", "ctx": {
//        "projectRoot", "config" } }
//        -- calls each plugin's `devMiddleware(ctx)` and accumulates
//           registrations; reply contains the (path, handlerId) pairs.
//
//   { "id": <number>, "kind": "devInvoke", "handlerId": "<id>",
//        "request": {...} }
//        -- dispatches one HTTP request to a previously-registered
//           dev-middleware handler; reply contains the response.
//
//   { "id": <number>, "kind": "virtualLoad", "loaderId": "<id>" }
//        -- invokes a virtual-module loader registered via setup's
//           addVirtualModule and returns the produced ESM source.
//           Loader output is cached after the first call so the same
//           specifier produces a byte-identical module on subsequent
//           imports.
//
//   { "id": <number>, "kind": "shutdown" }
//
// Each command receives one reply on stdout, also newline-delimited:
//
//   { "id": <number>, "ok": true, "result": <opaque> }
//   { "id": <number>, "ok": false, "error": {
//        "plugin": "<name>", "hook": "<hook>", "message": "<text>" } }
//
// Logger calls (`ctx.logger.info|warn|error`) emit standalone messages
// with no `id` and no reply expected:
//
//   { "log": { "level": "info|warn|error", "plugin": "<name>",
//              "message": "<text>" } }
//
// All envelopes are line-delimited; the Rust side splits on `\n`. We
// take pains never to embed a literal newline in a payload (JSON
// stringification escapes them as `\n` inside strings).

import process from "node:process";
import readline from "node:readline";

const { stdin, stdout, exit } = process;

let plugins = []; // [{ module, name, options, mod }] after init.
const devHandlers = new Map(); // handlerId -> { plugin, handler }
let nextHandlerId = 0;

// `setup` hook state (#255). `virtualLoaders` is keyed by the same
// opaque loaderId the Rust side stores on each `VirtualModuleEntry`;
// `virtualCache` memoises the first invocation's result so a loader
// runs exactly once per build / per dev-host boot, matching the
// "loader runs exactly once" contract documented in the spec.
const virtualLoaders = new Map(); // loaderId -> { plugin, loader }
const virtualCache = new Map(); // loaderId -> string (cached source)
let nextLoaderId = 0;

function send(envelope) {
  stdout.write(JSON.stringify(envelope) + "\n");
}

function sendOk(id, result) {
  send({ id, ok: true, result });
}

function sendErr(id, plugin, hook, err) {
  const message = err && err.stack ? err.stack : err && err.message ? err.message : String(err);
  send({ id, ok: false, error: { plugin, hook, message } });
}

function makeLogger(pluginName) {
  return {
    info(msg) {
      send({ log: { level: "info", plugin: pluginName, message: String(msg) } });
    },
    warn(msg) {
      send({ log: { level: "warn", plugin: pluginName, message: String(msg) } });
    },
    error(msg) {
      send({ log: { level: "error", plugin: pluginName, message: String(msg) } });
    },
  };
}

async function handleInit(id, msg) {
  // Load every plugin module up front. A failure on any one is fatal —
  // the build cannot run with a partially-initialised plugin set.
  plugins = [];
  for (const entry of msg.plugins ?? []) {
    let mod;
    try {
      mod = await import(entry.module);
    } catch (err) {
      sendErr(id, entry.name ?? entry.module, "init", err);
      return;
    }
    const def = mod && mod.default;
    if (!def || typeof def !== "object") {
      sendErr(
        id,
        entry.name ?? entry.module,
        "init",
        new Error(
          `plugin module ${entry.module} must \`export default\` a ZfbPlugin object (got ${typeof def})`,
        ),
      );
      return;
    }
    plugins.push({
      module: entry.module,
      // Prefer the plugin's self-declared name; fall back to the
      // user-config name; finally the module specifier itself.
      name: def.name ?? entry.name ?? entry.module,
      options: entry.options ?? {},
      mod: def,
    });
  }
  sendOk(id, { loaded: plugins.length });
}

async function runBuildHook(id, hookName, ctx) {
  // Run sequentially so plugin order in zfb.config.ts is meaningful
  // (later plugins can rely on earlier ones having finished). One
  // throw aborts the whole hook with a PluginError-shaped payload.
  for (const p of plugins) {
    const fn = p.mod[hookName];
    if (typeof fn !== "function") continue;
    const hookCtx = {
      projectRoot: ctx.projectRoot,
      outDir: ctx.outDir,
      config: ctx.config,
      options: p.options,
      logger: makeLogger(p.name),
    };
    // Thread the routes manifest through when present (#262). The Rust
    // side omits the field on preBuild so `ctx.routes` is undefined there.
    if (ctx.routes !== undefined && ctx.routes !== null) {
      hookCtx.routes = ctx.routes;
    }
    try {
      await fn.call(p.mod, hookCtx);
    } catch (err) {
      sendErr(id, p.name, hookName, err);
      return;
    }
  }
  sendOk(id, { ran: plugins.length });
}

async function handleSetup(id, msg) {
  // Reset accumulated state so a config reload (future feature)
  // doesn't surface stale loaders / routes from the previous run.
  virtualLoaders.clear();
  virtualCache.clear();
  nextLoaderId = 0;
  const command = msg.ctx && msg.ctx.command;
  if (command !== "build" && command !== "dev") {
    sendErr(
      id,
      "(host)",
      "setup",
      new Error(`setup ctx.command must be "build" or "dev" (got ${JSON.stringify(command)})`),
    );
    return;
  }
  const outputs = [];
  for (const p of plugins) {
    const fn = p.mod.setup;
    if (typeof fn !== "function") {
      // Silent no-op for plugins without `setup`. Still emit an
      // empty entry so the Rust side can reason about declaration
      // order if it wants to (it doesn't today, but the wire shape
      // stays simpler and the cost is negligible).
      outputs.push({ plugin: p.name, registrations: [] });
      continue;
    }
    const registrations = [];
    // Closed surface: only the four declared methods. We deliberately
    // do NOT expose addRemarkPlugin / addRehypePlugin / addMarkdownVisitor —
    // markdown extension is closed in v1 (see big-plan §B1 rationale).
    const ctx = {
      command,
      projectRoot: msg.ctx.projectRoot,
      config: msg.ctx.config,
      options: p.options,
      logger: makeLogger(p.name),
      addAlias(from, to) {
        if (typeof from !== "string" || from.length === 0) {
          throw new Error(
            `addAlias: \`from\` must be a non-empty string (got ${JSON.stringify(from)})`,
          );
        }
        if (typeof to !== "string" || to.length === 0) {
          throw new Error(
            `addAlias: \`to\` must be a non-empty string (got ${JSON.stringify(to)})`,
          );
        }
        registrations.push({ kind: "alias", from, to });
      },
      addVirtualModule(specifier, loader) {
        if (typeof specifier !== "string" || specifier.length === 0) {
          throw new Error(
            `addVirtualModule: \`specifier\` must be a non-empty string (got ${JSON.stringify(specifier)})`,
          );
        }
        if (typeof loader !== "function") {
          throw new Error(
            `addVirtualModule: \`loader\` must be a function returning a string or Promise<string> (got ${typeof loader})`,
          );
        }
        const loaderId = `v${nextLoaderId++}`;
        virtualLoaders.set(loaderId, { plugin: p.name, loader });
        registrations.push({ kind: "virtualModule", specifier, loaderId });
      },
      injectRoute(pattern, entrypoint, opts) {
        if (typeof pattern !== "string" || !pattern.startsWith("/")) {
          throw new Error(
            `injectRoute: \`pattern\` must be a string starting with "/" (got ${JSON.stringify(pattern)})`,
          );
        }
        // Reject obvious malformed patterns up front so the routers
        // don't have to: consecutive slashes are almost always a typo,
        // and empty bracket segments ([] / [...]) are not valid in the
        // pages/ grammar.
        //
        // The bare-"/" rejection is DEV-ONLY (#1193): in dev, "/" is the
        // catch-all owned by devMiddleware, so a dev injectRoute("/") is
        // a mistake. In a BUILD, a "/" package route is legitimate — it
        // materialises as the overlay `pages/index.tsx` (the project's
        // root page), enabling a truly empty/absent user `pages/`.
        if (command !== "build" && pattern === "/") {
          throw new Error(
            `injectRoute: \`pattern\` must not be "/" in dev (use devMiddleware for the catch-all) (got ${JSON.stringify(pattern)})`,
          );
        }
        if (pattern.includes("//")) {
          throw new Error(
            `injectRoute: \`pattern\` must not contain consecutive slashes (got ${JSON.stringify(pattern)})`,
          );
        }
        if (/\[\]|\[\.\.\.\]/.test(pattern)) {
          throw new Error(
            `injectRoute: \`pattern\` has an empty parameter segment (got ${JSON.stringify(pattern)})`,
          );
        }
        if (typeof entrypoint !== "string" || entrypoint.length === 0) {
          throw new Error(
            `injectRoute: \`entrypoint\` must be a non-empty string (got ${JSON.stringify(entrypoint)})`,
          );
        }
        // Optional third arg: `{ prerender?: boolean }` (#1193). The
        // prerender hint is build-only metadata (it controls whether the
        // overlay module inlines `export const prerender = …`); a dev
        // injectRoute may pass it harmlessly. Anything other than a
        // boolean (or absent) is rejected so a typo surfaces early.
        let prerender;
        if (opts != null) {
          if (typeof opts !== "object") {
            throw new Error(
              `injectRoute: \`opts\` must be an object (got ${JSON.stringify(opts)})`,
            );
          }
          if (opts.prerender !== undefined) {
            if (typeof opts.prerender !== "boolean") {
              throw new Error(
                `injectRoute: \`opts.prerender\` must be a boolean (got ${JSON.stringify(opts.prerender)})`,
              );
            }
            prerender = opts.prerender;
          }
        }
        registrations.push({ kind: "injectRoute", pattern, entrypoint, prerender });
      },
      addClientEntry(entrypoint) {
        if (typeof entrypoint !== "string" || entrypoint.length === 0) {
          throw new Error(
            `addClientEntry: \`entrypoint\` must be a non-empty string (got ${JSON.stringify(entrypoint)})`,
          );
        }
        // #1191 review [9]: enforce the `*.client.{ts,tsx,js,jsx}` convention
        // up front (the Rust accumulator re-validates via
        // `zfb_types::is_client_script_file` — these literals mirror
        // `crates/zfb-types/src/client_scripts.rs`, the contract's home).
        // Reject early so a preset author's typo surfaces here with the
        // entrypoint they wrote, not as an invented asset name downstream.
        const base = entrypoint.split(/[\\/]/).pop() ?? "";
        const isClientScript = ["ts", "tsx", "js", "jsx"].some((ext) => {
          const suffix = `.client.${ext}`;
          return base.endsWith(suffix) && base.length > suffix.length;
        });
        if (!isClientScript) {
          throw new Error(
            `addClientEntry: \`entrypoint\` must point to a *.client.{ts,tsx,js,jsx} file ` +
              `(the .client. infix is required and the stem before it must be non-empty); ` +
              `got ${JSON.stringify(entrypoint)}`,
          );
        }
        registrations.push({ kind: "addClientEntry", entrypoint });
      },
    };
    try {
      await fn.call(p.mod, ctx);
    } catch (err) {
      sendErr(id, p.name, "setup", err);
      return;
    }
    outputs.push({ plugin: p.name, registrations });
  }
  sendOk(id, { outputs });
}

async function handleVirtualLoad(id, msg) {
  const entry = virtualLoaders.get(msg.loaderId);
  if (!entry) {
    sendErr(
      id,
      "(unknown)",
      "setup",
      new Error(
        `virtualLoad: unknown loaderId ${JSON.stringify(msg.loaderId)} (was setup re-run?)`,
      ),
    );
    return;
  }
  // First-call wins: subsequent invocations return the memoised
  // source string verbatim. Matches the "loader runs exactly once
  // per build" contract from #255 and the cache check in the V8 host
  // (#260) / esbuild plugin (#261).
  if (virtualCache.has(msg.loaderId)) {
    sendOk(id, { source: virtualCache.get(msg.loaderId) });
    return;
  }
  let source;
  try {
    source = await entry.loader();
  } catch (err) {
    sendErr(id, entry.plugin, "setup", err);
    return;
  }
  if (typeof source !== "string") {
    sendErr(
      id,
      entry.plugin,
      "setup",
      new Error(`virtual module loader must return a string (got ${typeof source})`),
    );
    return;
  }
  virtualCache.set(msg.loaderId, source);
  sendOk(id, { source });
}

async function handleDevRegister(id, msg) {
  // Reset previous registrations so a config reload (future feature)
  // doesn't double-bind paths. The dev server expects exactly one
  // handler per (plugin, path); a plugin re-registering the same path
  // overwrites itself.
  devHandlers.clear();
  nextHandlerId = 0;
  const registrations = [];
  for (const p of plugins) {
    const fn = p.mod.devMiddleware;
    if (typeof fn !== "function") continue;
    const localPaths = new Map(); // path -> handlerId for THIS plugin
    const ctx = {
      projectRoot: msg.ctx.projectRoot,
      config: msg.ctx.config,
      options: p.options,
      logger: makeLogger(p.name),
      register(path, handler) {
        if (typeof path !== "string" || !path.startsWith("/")) {
          throw new Error(
            `devMiddleware register: path must be a string starting with "/" (got ${JSON.stringify(path)})`,
          );
        }
        if (typeof handler !== "function") {
          throw new Error(
            `devMiddleware register: handler must be a function (got ${typeof handler})`,
          );
        }
        let handlerId = localPaths.get(path);
        if (handlerId === undefined) {
          handlerId = `h${nextHandlerId++}`;
          localPaths.set(path, handlerId);
          registrations.push({ path, handlerId, plugin: p.name });
        }
        devHandlers.set(handlerId, { plugin: p.name, handler });
      },
    };
    try {
      await fn.call(p.mod, ctx);
    } catch (err) {
      sendErr(id, p.name, "devMiddleware", err);
      return;
    }
  }
  sendOk(id, { registrations });
}

async function handleDevInvoke(id, msg) {
  const entry = devHandlers.get(msg.handlerId);
  if (!entry) {
    sendErr(
      id,
      "(unknown)",
      "devMiddleware",
      new Error(`unknown handlerId ${JSON.stringify(msg.handlerId)}`),
    );
    return;
  }
  const { plugin, handler } = entry;
  let resp;
  try {
    resp = await handler(msg.request);
  } catch (err) {
    sendErr(id, plugin, "devMiddleware", err);
    return;
  }
  // Returning `undefined` means "I did not handle this request" — the
  // dev server falls through to its built-in routes. Encode as a
  // distinguished `passthrough: true` reply so the Rust side can
  // dispatch without an extra signal channel.
  if (resp === undefined || resp === null) {
    sendOk(id, { passthrough: true });
    return;
  }
  if (typeof resp.status !== "number") {
    sendErr(
      id,
      plugin,
      "devMiddleware",
      new Error(
        `devMiddleware handler must return { status, ... } or undefined (got ${typeof resp})`,
      ),
    );
    return;
  }
  sendOk(id, {
    passthrough: false,
    status: resp.status,
    headers: resp.headers ?? {},
    body: typeof resp.body === "string" ? resp.body : "",
    bodyEncoding: resp.bodyEncoding === "base64" ? "base64" : "utf8",
  });
}

const rl = readline.createInterface({ input: stdin });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch (err) {
    // Malformed input — fatal. The Rust side never lets a half-line
    // through, so this only fires on a programming error in the host.
    process.stderr.write(`[zfb-plugin-host] failed to parse line: ${err.message}\n`);
    exit(2);
    return;
  }
  // Each handler is async; return a promise but don't await — the
  // readline loop is sequential by line, but allowing concurrent
  // handler execution is fine because the protocol is request/reply
  // keyed by `id`.
  switch (msg.kind) {
    case "init":
      handleInit(msg.id, msg);
      break;
    case "setup":
      handleSetup(msg.id, msg);
      break;
    case "preBuild":
      runBuildHook(msg.id, "preBuild", msg.ctx);
      break;
    case "postBuild":
      runBuildHook(msg.id, "postBuild", msg.ctx);
      break;
    case "devRegister":
      handleDevRegister(msg.id, msg);
      break;
    case "devInvoke":
      handleDevInvoke(msg.id, msg);
      break;
    case "virtualLoad":
      handleVirtualLoad(msg.id, msg);
      break;
    case "shutdown":
      sendOk(msg.id, { bye: true });
      // Give the reply a tick to flush before exiting.
      setImmediate(() => exit(0));
      break;
    default:
      sendErr(
        msg.id ?? 0,
        "(host)",
        "(none)",
        new Error(`unknown kind ${JSON.stringify(msg.kind)}`),
      );
  }
});

rl.on("close", () => {
  // Parent closed our stdin — exit cleanly. (Matches the rl-on-shutdown
  // path so a kill-by-EOF is indistinguishable from a graceful one.)
  exit(0);
});
