#!/usr/bin/env node
/** Keep bundled starter decisions on the workshop's actual generation boundary.
 * Generated files are project-owned after scaffolding; users need no generator.
 * Run: node scripts/generate-starter-designs.mjs [--check]
 */
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import {
  createState,
  getSeedFiles,
  currentRules,
  windObject,
} from "../docs/src/components/playground/design-workshop/model.js";

export const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const templates = ["basic-blog", "node-free"];
export function starterAssets() {
  const state = createState("everyday");
  const files = getSeedFiles(state);
  const rules = currentRules(state);
  const ruleList = rules.map((rule) => "- " + rule).join("\n");
  return {
    "styles/design-system.css": files["design-system.css"],
    "design-tokens.json": JSON.stringify(windObject(state.values), null, 2) + "\n",
    "design-rules.json": JSON.stringify(rules, null, 2) + "\n",
    "DESIGN-NOTES.md": `# Everyday starter design\n\nThese are editable project-owned choices, not zudo-wind engine defaults.\n\n## Usage rules\n\n${ruleList}\n\n## Make it yours\n\nEdit styles/design-system.css for values and design-tokens.json for semantic\nnames. Basic blog merges those tokens into zfb.config.ts, retaining its Markdown\nshowcase palette. Node-free keeps its explicit Wind section in zfb.config.json;\nupdate it too if you rename a token. Update design-rules.json and these notes\nwhen changing values; the home page reads those rules. Neither starter needs\nthe workshop at runtime.\n\nThe accent chain is --ds-brand → --ds-accent → bg-accent. Body text uses\n--ds-font-body → text-body. Use px-hsp-card for horizontal padding and\npy-vsp-stack for vertical grouping; gap-x-hsp-gutter names column spacing\nindependently of gap-y-vsp-stack. max-w-reading limits prose, while\nrounded-panel shares the corner rule. Basic blog's local dark overrides live\nin styles/global.css and follow its existing theme toggle.\n\nExplore alternatives in the [Design system playground](https://zfb.takazudomodular.com/docs/playground/design-system/).\nChoosing another design in the playground does not change the initializer.\n`,
  };
}
export async function generate({ check = false, projectRoot = root } = {}) {
  const drift = [];
  const assets = starterAssets();
  for (const template of templates) {
    const directory = resolve(projectRoot, "crates/zfb/templates", template);
    const targets = { ...assets };
    if (template === "node-free") {
      const config = JSON.parse(await readFile(resolve(directory, "zfb.config.json"), "utf8"));
      config.wind = windObject(createState("everyday").values);
      targets["zfb.config.json"] = JSON.stringify(config, null, 2) + "\n";
    }
    for (const [name, content] of Object.entries(targets)) {
      const path = resolve(directory, name);
      if (check) {
        const actual = await readFile(path, "utf8").catch((error) => {
          if (error.code === "ENOENT") return null;
          throw error;
        });
        if (actual !== content) drift.push(`${template}/${name}`);
      } else {
        await mkdir(dirname(path), { recursive: true });
        await writeFile(path, content);
      }
    }
  }
  return drift;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const check = process.argv.includes("--check");
  const drift = await generate({ check });
  if (drift.length) {
    console.error("Starter design drift:\n" + drift.join("\n"));
    process.exitCode = 1;
  } else {
    console.log(
      check ? "Starter designs match the shared generator." : "Generated Everyday starter designs.",
    );
  }
}
