// @verification: one reviewed post-sync data repair, not a re-pin capability.
// Optional GitHub writes create UNREFERENCED blobs/tree only, never commits or refs.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync, mkdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";

const expected = {
  "crates/zfb-md-wasm/README.md": {
    before: "154f80d38a7f46b16c2eae63774274c8f55717d7e4cc4a8d7c5aa4640f6fa187",
    after: "28b85189f1b83c9ab28daf1f8ea8419093c04dc28ea5050a947379327d2644d1",
  },
  "crates/zfb-md-wasm/npm/README.md": {
    before: "0052a84a49e2e0420d62ae3b6ce9383e2358a13838961744ce303760a56c285c",
    after: "56009729961e9c88151a61365c7cf1060ec74b27a6471be9fddf8c2ecec888d8",
  },
  "crates/zfb-md-wasm/shipped-sizes.json": {
    before: "4fb8e180e8122e44ba33f445b889a787945cc316b1f792d3ec1a950e7529ec15",
    after: "306e950e31b1009ed8548392f5e97cb2d1f4980c63c1d51a27faaefaff02761d",
  },
  "docs/public/wind-examples/manifest.json": {
    before: "25c349606eb90a087db4b63a51244ce79bc634fed422b851e805038b13ebe14c",
    after: "1b8c31596be4eca3e589edd656619403c521b4827895b4be81112b444fb7cf03",
  },
  "docs/scripts/wind-compatibility-pages.mjs": {
    before: "436980c94928902e107239604493233d2caf5eccd3d69a72b90cd9bc20ed7f81",
    after: "4e4d7402b01d24cb234718059a55b317c510216fd529b23990d50104385f34e8",
  },
  "docs/src/content/docs-ja/api/md-wasm.mdx": {
    before: "92c4106814208bbdc8367ac9b95f3799161c05f670b4ece9caeea3015127ae7f",
    after: "64e2910c91fa7926ff4ea8b0f4c76c17a7a5e9664288b6a81fbde2422d1832ea",
  },
  "docs/src/content/docs-ja/guides/browser-markdown-preview.mdx": {
    before: "8e65baba28c67e6b107d01bc064e2ef8c9a50f45303f2ac7ea52d99adb1a8a6e",
    after: "3e254038b0099579b01d334b14a2e215c5910473140501a075bca9823d31bdc0",
  },
  "docs/src/content/docs-ja/guides/syntax-highlighting.mdx": {
    before: "80e47829c47400b766e76930c8bf5b229a34d42f4554d74c25e8379be8119cdd",
    after: "aeac07e029aa241564c6d42202335c86d3b86d17208261cc4ecf510d4c06c288",
  },
  "docs/src/content/docs/api/md-wasm.mdx": {
    before: "b930627c5f3d3ad02a29f095e00cbffb9c0f62ef1a867ebe3b49f62a5b2db39e",
    after: "4fa506121e58219eeb3ad9dc2d1d5cb748687c31fc8cbe1116f83647204d43c3",
  },
  "docs/src/content/docs/guides/browser-markdown-preview.mdx": {
    before: "152fef0cd8e16539799e7d7f097172683795569afcc16e82a9b2a9518150a20e",
    after: "d62e4d0d9fd41079240ef3d08e84c686a8e7620dd8218cd049621460f5ebc66a",
  },
  "docs/src/content/docs/guides/syntax-highlighting.mdx": {
    before: "5c660a02fed87d7853e23115c107a6fc95a039c5d6afdb3c2751faf7825c021a",
    after: "0d11958fa747626ecb042f1884e47537765a512282d3aca317491d761c4c26f8",
  },
};
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
assert.equal(
  hash(readFileSync("Cargo.lock")),
  "63aa8f330710941b8dfc4e5ccad81b0f42e2baf7c6e740ad42802223bc574c43",
);
assert.equal(
  hash(readFileSync("pnpm-lock.yaml")),
  "ee4accdbbbe22851c41ac529d492fbb6c03b007061ed0042068b2551731b5817",
);
const windPaths = [];
function walk(dir) {
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    const path = `${dir}/${item.name}`;
    if (item.isDirectory()) walk(path);
    else {
      assert.ok(item.isFile());
      windPaths.push(path);
    }
  }
}
walk("crates/zudo-wind");
windPaths.push("Cargo.toml", "Cargo.lock");
windPaths.sort();
assert.equal(windPaths.length, 100);
const wind = createHash("sha256");
for (const path of windPaths)
  wind.update(path).update("\0").update(readFileSync(path)).update("\0");
assert.equal(
  wind.digest("hex"),
  "5a08ea9a15e45de6afd6d7adfc6887e217e04d8249290a5ce0a1974b39aaee4c",
);
const names = Object.keys(expected);
const before = names.every((path) => hash(readFileSync(path)) === expected[path].before);
const after = names.every((path) => hash(readFileSync(path)) === expected[path].after);
assert.ok(before || after, "refuse stale, mixed, or unreviewed input files");
function replaceExactly(path, oldText, newText) {
  const text = readFileSync(path, "utf8");
  assert.equal(text.split(oldText).length, 2, `unique replacement required: ${path}`);
  writeFileSync(path, text.replace(oldText, newText));
}
if (before) {
  replaceExactly(
    "docs/scripts/wind-compatibility-pages.mjs",
    'export const REVIEWED_WIND_SOURCE_DIGEST =\n  "38ddeeefb6e867870d5714a49fa656909604951fc6125e5669882c475d2f8218";',
    '// #4120 post-sync: unchanged Wind sources/catalog; shared-lock dependency changes\n// verified by 117 library tests and 173 regenerated previews (no CSS byte changes).\n// Exact source/CI/artifact provenance: prototypes/rolldown/POST-SYNC.md.\nexport const REVIEWED_WIND_SOURCE_DIGEST =\n  "5a08ea9a15e45de6afd6d7adfc6887e217e04d8249290a5ce0a1974b39aaee4c";',
  );
  replaceExactly(
    "docs/public/wind-examples/manifest.json",
    "18fec169769dd6b2f446aeb8ce36a1cd4363c63517e6a95caf873e37f83e9daa",
    "3b25b7c131ea64514dcbff6cbe96798583f5227e67bcbdc2ea36187985f09aad",
  );
  const file = "crates/zfb-md-wasm/shipped-sizes.json";
  const data = JSON.parse(readFileSync(file, "utf8"));
  for (const [key, raw, gzip] of [
    ["root", 3389152, 1522818],
    ["highlight", 1540963, 821001],
    ["render", 2204415, 1097797],
    ["parse", 703502, 285765],
  ]) {
    data.measured[key].finalWasm = raw;
    data.measured[key].gzip9 = gzip;
  }
  writeFileSync(file, `${JSON.stringify(data, null, 2)}\n`);
  execFileSync(process.execPath, ["scripts/assert-md-wasm-size-docs.mjs", "--fix"], {
    stdio: "inherit",
  });
}
for (const path of names)
  assert.equal(hash(readFileSync(path)), expected[path].after, `reviewed output bytes: ${path}`);
execFileSync(process.execPath, ["scripts/assert-md-wasm-size-docs.mjs"], { stdio: "inherit" });
for (const locale of ["en", "ja"])
  execFileSync(
    process.execPath,
    ["docs/scripts/generate-wind-reference.mjs", "--locale", locale, "--check"],
    { stdio: "inherit" },
  );
console.log(
  `Verified exact reviewed bytes for ${names.length} generated/source-provenance files; island contracts untouched.`,
);
if (process.argv.includes("--prepare-tree")) {
  assert.equal(process.env.GITHUB_REPOSITORY, "Takazudo/zudo-front-builder");
  assert.equal(process.env.GITHUB_REF, "refs/heads/prototype/318-rolldown-native");
  const head = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  assert.equal(head, process.env.GITHUB_SHA);
  const changed = execFileSync("git", ["diff", "--name-only"], { encoding: "utf8" })
    .trim()
    .split("\n")
    .filter(Boolean)
    .sort();
  const formatPaths = [
    "prototypes/rolldown/post-sync-data.mjs",
    "prototypes/rolldown/plugin-prebundle.mjs",
    "prototypes/rolldown/POST-SYNC.md",
    "crates/zfb-rolldown-prototype/examples/plugin_prebundle.rs",
    ".github/workflows/rolldown-post-sync-data.yml",
  ];
  for (const path of changed)
    assert.ok(
      names.includes(path) || formatPaths.includes(path),
      `unreviewed changed path: ${path}`,
    );
  for (const path of names) assert.ok(changed.includes(path), `missing generated repair: ${path}`);
  const baseTree = execFileSync("git", ["rev-parse", "HEAD^{tree}"], { encoding: "utf8" }).trim();
  async function post(kind, body) {
    assert.ok(["blobs", "trees"].includes(kind));
    const response = await fetch(
      `https://api.github.com/repos/Takazudo/zudo-front-builder/git/${kind}`,
      {
        method: "POST",
        headers: {
          authorization: `Bearer ${process.env.GITHUB_TOKEN}`,
          accept: "application/vnd.github+json",
          "content-type": "application/json",
          "x-github-api-version": "2022-11-28",
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(30000),
      },
    );
    if (!response.ok) throw new Error(`Git object ${kind} creation failed: ${response.status}`);
    const result = await response.json();
    assert.match(result.sha, /^[a-f0-9]{40}$/);
    return result.sha;
  }
  const entries = [];
  for (const path of changed)
    entries.push({
      path,
      mode: "100644",
      type: "blob",
      sha: await post("blobs", { content: readFileSync(path, "utf8"), encoding: "utf-8" }),
    });
  const tree = await post("trees", { base_tree: baseTree, tree: entries });
  const output = join(process.env.RUNNER_TEMP, "rolldown-reviewed-tree");
  mkdirSync(output, { recursive: true });
  writeFileSync(
    join(output, "receipt.json"),
    `${JSON.stringify({ head, baseTree, tree, entries, expected, refUpdated: false, commitCreated: false }, null, 2)}\n`,
  );
  console.log(`Prepared unreferenced tree ${tree}; no commit or branch changed.`);
}
