// AUTO-LOADED by zfb-build's plugin runner (Phase B Sub 3 / issue #108).
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
    try {
      await fn.call(p.mod, hookCtx);
    } catch (err) {
      sendErr(id, p.name, hookName, err);
      return;
    }
  }
  sendOk(id, { ran: plugins.length });
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
