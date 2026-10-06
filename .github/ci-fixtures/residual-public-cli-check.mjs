import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
const [binaryArg, outputArg] = process.argv.slice(2);
const binary = resolve(binaryArg),
  output = resolve(outputArg);
mkdirSync(output, { recursive: true });
const project = mkdtempSync(join(tmpdir(), "zfb-content-alignment-"));
mkdirSync(join(project, "pages"));
mkdirSync(join(project, "dist"));
writeFileSync(join(project, "pages/index.html"), '<div class="block content-center">Probe</div>\n');
writeFileSync(
  join(project, "entry.css"),
  "/* authored entry intentionally has no align-content rule */\n",
);
writeFileSync(join(output, "index.html"), readFileSync(join(project, "pages/index.html")));
writeFileSync(join(output, "entry.css"), readFileSync(join(project, "entry.css")));
const results = [];
function command(name, args) {
  const r = spawnSync(binary, args, {
    cwd: project,
    encoding: "utf8",
    timeout: 30000,
    env: { ...process.env, PATH: "", NODE_PATH: "" },
  });
  assert.equal(r.error, undefined, `${name}: ${r.error}`);
  assert.equal(r.signal, null, `${name}: signal`);
  writeFileSync(join(output, `${name}.stdout`), r.stdout);
  writeFileSync(join(output, `${name}.stderr`), r.stderr);
  results.push({
    name,
    args,
    exitCode: r.status,
    stdoutSha256: createHash("sha256").update(r.stdout).digest("hex"),
    stderrSha256: createHash("sha256").update(r.stderr).digest("hex"),
  });
  return r;
}
for (const mode of ["default", "strict", "reserved"]) {
  const config = {
    wind: {
      spec: 1,
      strict: mode !== "default",
      ...(mode === "reserved" ? { authoredClasses: { "content-center": true } } : {}),
    },
  };
  writeFileSync(join(project, "zfb.config.json"), JSON.stringify(config));
  writeFileSync(join(output, `${mode}-config.json`), JSON.stringify(config));
  for (const candidate of [
    "content-center",
    "content-none",
    "content-area",
    "place-content-center",
  ]) {
    const name = `${mode}-${candidate}`;
    const r = command(name, [
      "wind",
      "explain",
      "--json",
      "--project-root",
      project,
      "--",
      candidate,
    ]);
    assert.equal(r.status, 0, name);
    const doc = JSON.parse(r.stdout);
    assert.equal(doc.schemaVersion, 1);
    assert.equal(doc.command, "explain");
    assert.equal(doc.explanations.length, 1);
    const e = doc.explanations[0];
    assert.equal(e.candidate, candidate);
    assert.equal(e.specVersion, 1);
    assert.equal(e.specRevision, 14);
    if (candidate === "content-center" && mode !== "reserved") {
      assert.equal(e.outcome, "foreign_utility");
      assert.deepEqual(e.declarations, []);
      assert.equal(e.diagnostics.length, 1);
      assert.equal(e.diagnostics[0].code, "ZW014");
      assert.equal(e.diagnostics[0].severity, mode === "default" ? "warning" : "error");
      assert.match(e.diagnostics[0].message, /align-content declaration/);
      assert.doesNotMatch(e.diagnostics[0].message, /::before|::after/);
    } else if (candidate === "content-none") {
      assert.equal(e.outcome, "foreign_utility");
      assert.deepEqual(e.declarations, []);
      assert.match(e.diagnostics[0].message, /content declaration on a ::before or ::after rule/);
      assert.doesNotMatch(e.diagnostics[0].message, /align-content/);
    } else if (candidate === "place-content-center") {
      assert.equal(e.outcome, "resolved_utility");
      assert.ok(e.declarations.some((x) => x.property === "place-content" && x.value === "center"));
    } else {
      assert.equal(e.outcome, "ordinary");
      assert.deepEqual(e.declarations, []);
      assert.deepEqual(e.diagnostics, []);
    }
  }
  const cssPath = join(project, "dist", `${mode}.css`);
  const r = command(`${mode}-css`, [
    "css",
    "--input",
    "entry.css",
    "--output",
    cssPath,
    "--project-root",
    project,
  ]);
  if (mode === "strict") {
    assert.notEqual(r.status, 0);
    assert.match(r.stdout + r.stderr, /ZW014/);
  } else {
    assert.equal(r.status, 0, `${mode} CSS: ${r.stderr}`);
    const css = readFileSync(cssPath, "utf8");
    writeFileSync(join(output, `${mode}.css`), css);
    assert.match(css, /\.block\s*\{/);
    assert.doesNotMatch(css, /\.content-center\s*\{/);
    assert.doesNotMatch(css, /align-content\s*:/);
    if (mode === "default") assert.match(r.stdout + r.stderr, /ZW014/);
    else assert.doesNotMatch(r.stdout + r.stderr, /ZW014/);
  }
}
writeFileSync(
  join(output, "summary.json"),
  JSON.stringify(
    {
      schemaVersion: 1,
      binary,
      binarySha256: createHash("sha256").update(readFileSync(binary)).digest("hex"),
      environment: { PATH: "", NODE_PATH: "" },
      project,
      results,
      passed: true,
    },
    null,
    2,
  ) + "\n",
);
console.log(`Verified ${results.length} public CLI cases`);
