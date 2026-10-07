#!/usr/bin/env node

// A built-CLI spot check for the Tailwind default-theme migration case (#3682).
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { spawnSync } from "node:child_process";

const binary = process.argv[2];
assert(binary && isAbsolute(binary), "pass the absolute path to the built zfb binary");

const root = mkdtempSync(join(tmpdir(), "zfb-zw016-audit-"));
try {
  mkdirSync(join(root, "src/styles"), { recursive: true });
  writeFileSync(join(root, "package.json"), "{}\n");
  writeFileSync(
    join(root, "zfb.config.json"),
    '{"wind":{"spec":1,"tokens":{"fontWeights":{"normal":"400"}}}}\n',
  );
  writeFileSync(join(root, "src/a.tsx"), 'export default () => <div class="block" />;\n');
  writeFileSync(
    join(root, "src/styles/global.css"),
    '@import "./tokens.css";\n.a { font-weight: var(--font-weight-normal); font-size: var(--text-2xl); }\n.b { gap: var(--spacing); font-weight: var(--font-weight-normal); }\n.c { font-weight: var(--font-weight-normal, 400); }\n',
  );
  writeFileSync(join(root, "src/styles/tokens.css"), ":root { --other: 1; }\n");

  const result = spawnSync(binary, ["wind", "audit", "--project-root", root], {
    cwd: root,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  const rows = result.stdout
    .split(/\r?\n/)
    .filter((line) => line.includes("ZW016 auditInfo at src/styles/global.css:"));
  assert.equal(rows.length, 4, result.stdout);
  assert.equal(rows.filter((line) => line.includes("--font-weight-normal")).length, 2);
  assert.equal(rows.filter((line) => line.includes("--text-2xl")).length, 1);
  assert.equal(rows.filter((line) => line.includes("--spacing")).length, 1);

  console.log("ZW016 built-CLI spot check:");
  for (const row of rows) console.log(row);
} finally {
  rmSync(root, { recursive: true, force: true });
}
