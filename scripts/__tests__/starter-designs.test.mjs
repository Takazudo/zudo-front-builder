import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { generate, root, starterAssets, templates } from "../generate-starter-designs.mjs";
import {
  createState,
  getSeedFiles,
  currentRules,
  windObject,
} from "../../docs/src/components/playground/design-workshop/model.js";

const temporary = [];
afterEach(async () => {
  await Promise.all(temporary.splice(0).map((path) => rm(path, { recursive: true, force: true })));
});

describe("bundled starter design boundary", () => {
  it("committed assets match the shared Everyday exporter and current rules", async () => {
    expect(await generate({ check: true })).toEqual([]);
    const state = createState("everyday");
    const assets = starterAssets();
    expect(assets["styles/design-system.css"]).toBe(getSeedFiles(state)["design-system.css"]);
    expect(JSON.parse(assets["design-tokens.json"])).toEqual(windObject(state.values));
    expect(
      JSON.parse(
        assets["components/design-rules.ts"].replace(/^export default /, "").replace(/;\n$/, ""),
      ),
    ).toEqual(currentRules(state));
  });

  it("checks fail for stale values, token roles, rules and the actual Node-free config", async () => {
    const projectRoot = await mkdtemp(resolve(tmpdir(), "zfb-starter-design-"));
    temporary.push(projectRoot);
    const configPath = resolve(projectRoot, "crates/zfb/templates/node-free/zfb.config.json");
    await mkdir(resolve(configPath, ".."), { recursive: true });
    await writeFile(
      configPath,
      '{"collections":[{"name":"posts","path":"content/posts"}],"outDir":"site"}\n',
    );
    await generate({ projectRoot });
    expect(await generate({ check: true, projectRoot })).toEqual([]);
    const config = JSON.parse(await readFile(configPath, "utf8"));
    expect(config.outDir).toBe("site");
    expect(config.collections).toEqual([{ name: "posts", path: "content/posts" }]);
    config.wind.tokens.spacing["hsp-card"] = "1px";
    await writeFile(configPath, JSON.stringify(config, null, 2) + "\n");
    for (const template of templates) {
      for (const name of Object.keys(starterAssets())) {
        await writeFile(resolve(projectRoot, "crates/zfb/templates", template, name), "stale\n");
      }
    }
    const drift = await generate({ check: true, projectRoot });
    expect(drift).toContain("node-free/zfb.config.json");
    for (const template of templates) {
      for (const name of Object.keys(starterAssets()))
        expect(drift).toContain(`${template}/${name}`);
    }
  });

  it("keeps the Node-free config explicit without adding an install contract", async () => {
    const directory = resolve(root, "crates/zfb/templates/node-free");
    const config = JSON.parse(await readFile(resolve(directory, "zfb.config.json"), "utf8"));
    expect(config.wind).toEqual(JSON.parse(starterAssets()["design-tokens.json"]));
    await expect(readFile(resolve(directory, "package.json"))).rejects.toMatchObject({
      code: "ENOENT",
    });
  });
});
