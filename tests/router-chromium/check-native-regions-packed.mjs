// Run after the ordinary package builds. No workspace links, copied runtime or patched node_modules.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildNativeRegions } from "./build-native-regions-fixture.mjs";

const repo = fileURLToPath(new URL("../../", import.meta.url));
const isolated = mkdtempSync(join(tmpdir(), "zfb-native-regions-packed-"));
const tarballs = join(isolated, "tarballs");
const fixture = join(isolated, "fixture");
mkdirSync(tarballs);
mkdirSync(fixture);
const run = (command, args, cwd = repo, env = process.env) =>
  execFileSync(command, args, { cwd, env, stdio: "inherit" });
for (const name of ["zfb-slugify", "zfb", "zfb-runtime"]) {
  run("pnpm", ["--dir", `packages/${name}`, "pack", "--pack-destination", tarballs]);
}
const archives = readdirSync(tarballs).filter((name) => name.endsWith(".tgz"));
assert.equal(archives.length, 3);
const dependencies = Object.fromEntries(
  archives.map((name) => {
    const metadata = JSON.parse(
      execFileSync("tar", ["-xOf", join(tarballs, name), "package/package.json"], {
        encoding: "utf8",
      }),
    );
    return [metadata.name, `file:${join(tarballs, name)}`];
  }),
);
writeFileSync(
  join(isolated, "package.json"),
  JSON.stringify({ private: true, type: "module", dependencies }),
);
run(
  "npm",
  [
    "install",
    "--ignore-scripts",
    "--omit=optional",
    "--package-lock=false",
    "--no-audit",
    "--no-fund",
  ],
  isolated,
);
const consumer = join(isolated, "consumer.mjs");
writeFileSync(
  consumer,
  `
import { Island } from '@takazudo/zfb';
import * as react from '@takazudo/zfb/zudo-react';
import { renderToString } from '@takazudo/zfb/zudo-react/server';
import { ClientRouter } from '@takazudo/zfb-runtime';
export { Island, react, renderToString, ClientRouter };
export const sdk = import.meta.resolve('@takazudo/zfb/runtime');
export const runtime = import.meta.resolve('@takazudo/zfb-runtime/client-router');
`,
);
const api = await import(pathToFileURL(consumer).href);
const sdkDist = dirname(fileURLToPath(api.sdk));
const runtimeDist = dirname(dirname(fileURLToPath(api.runtime)));
assert.ok(sdkDist.startsWith(join(isolated, "node_modules")));
assert.ok(runtimeDist.startsWith(join(isolated, "node_modules")));
assert.ok(api.sdk.endsWith("/dist/runtime.js"));
assert.ok(api.runtime.endsWith("/dist/client-router/index.js"));
buildNativeRegions({ ...api, fixture });
for (const name of ["native-regions.css", "real-islands-components.mjs"])
  cpSync(join(repo, "tests/router-chromium/fixture", name), join(fixture, name));
let bootstrap = readFileSync(
  join(repo, "tests/router-chromium/fixture/real-islands-bootstrap.js"),
  "utf8",
);
bootstrap = bootstrap
  .replace('"/dist/client-router/router.js"', '"@takazudo/zfb-runtime/client-router"')
  .replace('"/zfb-dist/zudo-react/client.js"', '"@takazudo/zfb/zudo-react/client"')
  .replace('"/zfb-dist/zudo-react/index.js"', '"@takazudo/zfb/zudo-react"');
writeFileSync(join(fixture, "real-islands-bootstrap.js"), bootstrap);
for (const file of readdirSync(fixture).filter((name) => name.endsWith(".html"))) {
  let html = readFileSync(join(fixture, file), "utf8");
  html = html.replace(
    '<meta name="zfb-view-transitions-enabled" content=""><meta name="zfb-view-transitions-fallback" content="animate">',
    api.renderToString(api.ClientRouter({})),
  );
  html = html.replace(
    '"@takazudo/zfb/runtime":"/zfb-dist/runtime.js"',
    '"@takazudo/zfb/runtime":"/zfb-dist/runtime.js","@takazudo/zfb-runtime/client-router":"/dist/client-router/index.js","@takazudo/zfb/zudo-react/client":"/zfb-dist/zudo-react/client.js","@takazudo/zfb/zudo-react":"/zfb-dist/zudo-react/index.js","@takazudo/zfb/zudo-react/jsx-runtime":"/zfb-dist/zudo-react/jsx-runtime.js"',
  );
  writeFileSync(join(fixture, file), html);
}
cpSync(join(fixture, "real-islands-region-a.html"), join(fixture, "index.html"));
const output = join(repo, "test-results", "native-regions-packed");
const evidence = {
  isolated,
  source: execFileSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8" }).trim(),
  node: process.version,
  sdkEntry: api.sdk,
  runtimeEntry: api.runtime,
  archives: archives.map((name) => ({
    name,
    sha256: createHash("sha256")
      .update(readFileSync(join(tarballs, name)))
      .digest("hex"),
  })),
};
writeFileSync(join(isolated, "packed-evidence.json"), JSON.stringify(evidence, null, 2));
console.log(JSON.stringify(evidence, null, 2));
// Keep results separate from the workspace run. The optional config is a local
// browser path override only; CI uses the repository's pinned Playwright browser.
try {
  run(
    "pnpm",
    [
      "exec",
      "playwright",
      "test",
      "--config",
      process.env.ZFB_REGION_PLAYWRIGHT_CONFIG ?? "tests/router-chromium/playwright.config.mjs",
      "native-regions",
      "--output",
      output,
    ],
    repo,
    {
      ...process.env,
      ZFB_REGION_FIXTURE_DIR: fixture,
      ZFB_REGION_SDK_DIST: sdkDist,
      ZFB_REGION_RUNTIME_DIST: runtimeDist,
    },
  );
} finally {
  mkdirSync(output, { recursive: true });
  writeFileSync(join(output, "packed-evidence.json"), JSON.stringify(evidence, null, 2));
}
console.log(`Packed native region evidence: ${output}; tarballs: ${isolated}`);
