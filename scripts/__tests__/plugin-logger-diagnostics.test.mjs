import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";
import { it } from "vite-plus/test";

// Exercise the real lightweight Node protocol, without building Rust or a site.
it("plugin logger accepts legacy calls and forwards additive metadata", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zfb-log-schema-"));
  let child;
  try {
    const plugin = join(dir, "plugin.mjs");
    await writeFile(
      plugin,
      `export default {
    name: "fixture",
    preBuild({logger}) {
      logger.info("legacy");
      logger.warn("located", {code:"fixture/missing", sourceId:"authored", file:"日本.ts", line:2, byteColumn:7, column:3, level:"error", plugin:"spoof", message:"spoof"});
      logger.error("bad metadata", {line:0, byteColumn:1.5, code:12});
    }
  };`,
    );
    child = spawn(
      process.execPath,
      [fileURLToPath(new URL("../../crates/zfb/js/plugin-host.mjs", import.meta.url))],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    let stderr = "";
    child.stderr.on("data", (data) => {
      stderr += data;
    });
    const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
    const send = (value) => child.stdin.write(JSON.stringify(value) + "\n");
    const read = async () => {
      const { value, done } = await lines.next();
      assert.equal(done, false, stderr);
      return JSON.parse(value);
    };
    send({
      id: 1,
      kind: "init",
      plugins: [{ name: "fixture", module: pathToFileURL(plugin).href, options: {} }],
    });
    assert.equal((await read()).ok, true, stderr);
    send({
      id: 2,
      kind: "preBuild",
      ctx: {
        projectRoot: dir,
        outDir: join(dir, "dist"),
        scratchDir: join(dir, "scratch"),
        config: {},
      },
    });
    assert.deepEqual(await read(), {
      log: { level: "info", plugin: "fixture", message: "legacy" },
    });
    assert.deepEqual(await read(), {
      log: {
        level: "warn",
        plugin: "fixture",
        message: "located",
        code: "fixture/missing",
        sourceId: "authored",
        file: "日本.ts",
        line: 2,
        byteColumn: 7,
      },
    });
    assert.deepEqual(await read(), {
      log: { level: "error", plugin: "fixture", message: "bad metadata" },
    });
    assert.equal((await read()).ok, true, stderr);
    const exited = once(child, "exit");
    send({ id: 3, kind: "shutdown" });
    assert.equal((await read()).ok, true, stderr);
    assert.equal((await exited)[0], 0, stderr);
  } finally {
    if (child && child.exitCode === null && child.signalCode === null) {
      const closed = new Promise((resolve) => child.once("close", resolve));
      child.kill();
      await closed;
    }
    await rm(dir, { recursive: true, force: true });
  }
}, 10000);
