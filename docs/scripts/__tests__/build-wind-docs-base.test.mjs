import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configAtBase, buildAtBase } from "../build-wind-docs-base.mjs";

const source = 'export default zudoDoc({\n    base: "/",\n});\n';

test("prefix builds configure the base and flat public files for a whole-tree mount", async () => {
  const configured = configAtBase(
    `const zudoDoc = (settings) => ({ ...settings, copyPublicWithBase: true });\n${source}`,
    "/wind-docs-preview/",
  );
  const { default: config } = await import(
    `data:text/javascript,${encodeURIComponent(configured)}`
  );
  assert.equal(config.base, "/wind-docs-preview/");
  assert.equal(config.copyPublicWithBase, false);
  for (const base of ["/", "../escape/", "/prefix?query/", '";process.exit();//']) {
    assert.throws(() => configAtBase(source, base));
  }
  assert.throws(() => configAtBase(source + source, "/prefix/"), /exactly one/);
  assert.throws(
    () => configAtBase(source.replace("export default ", "const config = "), "/prefix/"),
    /exactly one default export/,
  );
  assert.throws(
    () => configAtBase(`const __windDocsSubpathConfig = {};\n${source}`, "/prefix/"),
    /reserved preview binding/,
  );
});

test("config is restored after successful, failed, and throwing builds", () => {
  const dir = mkdtempSync(join(tmpdir(), "wind-docs-base-test-"));
  const configPath = join(dir, "config.ts");
  try {
    for (const outcome of ["success", "failure", "throw"]) {
      writeFileSync(configPath, source);
      const run = (command, args) => {
        assert.equal(command, "pnpm");
        assert.deepEqual(args, ["--filter", "docs", "build"]);
        assert.match(readFileSync(configPath, "utf8"), /base: "\/prefix\/"/);
        assert.match(readFileSync(configPath, "utf8"), /copyPublicWithBase: false/);
        if (outcome === "throw") throw new Error("launch failed");
        return { status: outcome === "success" ? 0 : 1 };
      };
      if (outcome === "success") buildAtBase("/prefix/", { configPath, run });
      else assert.throws(() => buildAtBase("/prefix/", { configPath, run }));
      assert.equal(readFileSync(configPath, "utf8"), source);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
