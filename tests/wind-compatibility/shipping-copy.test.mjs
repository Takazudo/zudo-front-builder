import assert from "node:assert/strict";
import { cp, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fixtureCopyFilter } from "./shipping/fixture-copy.mjs";

test("a clean shipping fixture retains package dist sources while dropping root output", async () => {
  const temp = await mkdtemp(join(tmpdir(), "zfb-shipping-copy-"));
  const source = join(temp, "source");
  const clean = join(temp, "clean");
  try {
    await cp(new URL("./shipping/fixtures/cli/", import.meta.url), source, { recursive: true });
    await mkdir(join(source, "dist"));
    await writeFile(join(source, "dist", "generated.css"), "stale");
    await writeFile(join(source, "generated.css"), "stale");
    await cp(source, clean, {
      recursive: true,
      filter: fixtureCopyFilter(source, { excludeCssOutputs: true }),
    });
    assert.match(
      await readFile(join(clean, "node_modules/@fixture/shipping-ui/dist/component.js"), "utf8"),
      /text-brand/,
    );
    assert.match(
      await readFile(join(clean, "node_modules/@fixture/shipping-ui/wind.json"), "utf8"),
      /bg-manifest/,
    );
    await assert.rejects(readFile(join(clean, "dist/generated.css")), { code: "ENOENT" });
    await assert.rejects(readFile(join(clean, "generated.css")), { code: "ENOENT" });
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});
