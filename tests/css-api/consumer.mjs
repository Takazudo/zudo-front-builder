import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";
import { compileCss, CssCompileError } from "@takazudo/zfb/css";
import { definePreset } from "@takazudo/zfb/config";
const require = createRequire(import.meta.url);
const room = process.env.ZFB_CSS_API_ROOM;
const repo = process.env.ZFB_CSS_API_REPO;
const fixtures = path.join(repo, "crates/zfb/tests/fixtures");
for (const name of ["zfb", "zfb-slugify", "zfb-linux-x64-gnu"]) {
  const resolved = fs.realpathSync(
    require.resolve(`@takazudo/${name}${name === "zfb-slugify" ? "" : "/package.json"}`),
  );
  assert(resolved.startsWith(room + "/"), `${name} escaped clean room: ${resolved}`);
}
const carrier = path.dirname(require.resolve("@takazudo/zfb-linux-x64-gnu/package.json"));
assert.equal(fs.statSync(path.join(carrier, "zfb")).mode & 0o777, 0o755);
fs.unlinkSync(path.join(carrier, "zfb")); // the API has no executable to fall back to
function write(root, file, text) {
  fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
  fs.writeFileSync(path.join(root, file), text);
}
function fixture(name) {
  const root = path.join(room, name);
  fs.cpSync(path.join(fixtures, name), root, { recursive: true });
  return root;
}
const cwdBefore = process.cwd();
const deterministic = fixture("css-determinism");
write(deterministic, "zfb.config.ts", 'throw new Error("CONFIG MUST NOT BE EVALUATED");');
const golden = await compileCss({ cwd: deterministic, input: "entry.css", sources: [] });
assert.equal(golden.css, fs.readFileSync(path.join(deterministic, "golden.css"), "utf8"));
assert.deepEqual(golden.diagnostics, []);
assert.deepEqual(golden.warnings, []);
fs.unlinkSync(path.join(deterministic, "zfb.config.ts"));
execFileSync(
  process.env.ZFB_CSS_API_CLI,
  ["css", "--input", "entry.css", "--output", "actual.css"],
  { cwd: deterministic },
);
assert.equal(golden.css, fs.readFileSync(path.join(deterministic, "actual.css"), "utf8"));
assert(
  !(await compileCss({ cwd: deterministic, input: "entry.css", noAutoSource: true })).css.includes(
    ".flex",
  ),
);
const root = path.join(room, "anchors");
write(
  root,
  "invoke/input.css",
  '@import "./part.css"; @import "@fixture/styles"; .authored { color: red }',
);
write(root, "invoke/part.css", ".imported { display: block }");
write(
  root,
  "invoke/node_modules/@fixture/styles/package.json",
  JSON.stringify({ name: "@fixture/styles", exports: { ".": "./style.css" } }),
);
write(root, "invoke/node_modules/@fixture/styles/style.css", ".packaged { color: blue }");

write(root, "project/src/auto.html", '<div class="flex"></div>');
write(root, "project/explicit.html", '<div class="grid"></div>');
write(root, "project/out/excluded.html", '<div class="hidden"></div>');
write(root, "config/templates/keep.html", '<div class="block"></div>');
write(root, "config/templates/skip.html", '<div class="inline"></div>');
const options = {
  cwd: path.join(root, "invoke"),
  input: "input.css",
  projectRoot: "../project",
  configDir: "../config",
  noAutoSource: true,
  sources: ["explicit.html"],
  config: {
    outDir: "out",
    wind: {
      sources: { roots: ["templates"], exclude: ["templates/skip.html"] },
      safelist: { test: ["underline"] },
    },
    codeHighlight: { classPrefix: undefined },
  },
};
const anchored = await compileCss(options);
for (const selector of [".imported", ".packaged", ".grid", ".block", ".underline"])
  assert(anchored.css.includes(selector), selector);
for (const selector of [".flex", ".hidden", ".inline {"])
  assert(!anchored.css.includes(selector), selector);
const explicit = await compileCss({
  ...options,
  sources: ["**/*.html"],
  output: "../project/result.css",
});
assert(explicit.css.includes(".grid"));
assert.equal(explicit.css, fs.readFileSync(path.join(root, "project/result.css"), "utf8"));
assert(explicit.warnings.some((w) => w.includes("excluded")));
const disabled = await compileCss({
  ...options,
  sources: ["nonexistent"],
  config: { wind: false, plugins: { irrelevant: () => {} }, site: () => {} },
});
assert(disabled.css.includes(".authored"));
assert(!disabled.css.includes(".grid"));
await assert.rejects(
  compileCss({ ...options, sources: ["../invoke/input.css"] }),
  (error) =>
    error instanceof CssCompileError &&
    error.code === "ZW010" &&
    error.severity === "error" &&
    !!error.origin.path,
);
await assert.rejects(
  compileCss({ ...options, config: { wind: { tokens: { fontSizes: { bad: "12px" } } } } }),
  /fontSizes.*bad/,
);
write(root, "invoke/bad.css", '@import "./missing.css";');
write(root, "invoke/prior.css", "previous output");
write(root, "project/invalid.html", '<div class="bg-missing"></div>');
await assert.rejects(
  compileCss({ ...options, sources: ["invalid.html"], output: "prior.css" }),
  (error) =>
    error instanceof CssCompileError &&
    error.code === "ZW006" &&
    error.severity === "error" &&
    error.origin.line === 1 &&
    error.origin.column > 0,
);
fs.unlinkSync(path.join(root, "project/invalid.html"));

await assert.rejects(compileCss({ ...options, input: "bad.css", output: "prior.css" }), /missing/);
assert.equal(fs.readFileSync(path.join(root, "invoke/prior.css"), "utf8"), "previous output");
await assert.rejects(compileCss({ ...options, output: "input.css" }), /same/);
assert((await compileCss(options)).css.includes(".grid"));
// Real filesystem resolution of link/.. differs from lexical collapse.
write(root, "physical/nested/placeholder", "");
write(root, "physical/entry.css", ".physical { color: blue }");
write(root, "physical/src/a.html", '<i class="inline-flex"></i>');
fs.symlinkSync(path.join(root, "physical/nested"), path.join(root, "invoke/link"), "dir");
const symlink = await compileCss({
  cwd: path.join(root, "invoke"),
  input: "link/../entry.css",
  projectRoot: "link/..",
});
assert(symlink.css.includes(".physical"));
assert(symlink.css.includes(".inline-flex"));
const concurrent = await Promise.all(
  Array.from({ length: 8 }, (_, i) => {
    const ownRoot = path.join(root, `concurrent-${i}`);
    write(ownRoot, "invoke/entry.css", `.request-${i} { color: red }`);
    write(ownRoot, "project/explicit.html", `<i class="${i % 2 ? "flex" : "grid"}"></i>`);
    write(ownRoot, "config/templates/source.html", '<i class="underline"></i>');
    return compileCss({
      cwd: path.join(ownRoot, "invoke"),
      input: "entry.css",
      projectRoot: "../project",
      configDir: "../config",
      noAutoSource: true,
      sources: ["explicit.html"],
      output: "../project/result.css",
      config: {
        wind: {
          sources: { roots: ["templates"] },
          safelist: { concurrent: [i % 2 ? "flex" : "grid"] },
        },
      },
    });
  }),
);
concurrent.forEach((result, i) => {
  assert(result.css.includes(i % 2 ? ".flex" : ".grid"));
  assert(!result.css.includes(i % 2 ? ".grid" : ".flex"));
  assert(result.css.includes(`.request-${i}`));
  assert(result.css.includes(".underline"));
  assert.equal(
    result.css,
    fs.readFileSync(path.join(root, `concurrent-${i}/project/result.css`), "utf8"),
  );
});
const highlighted = fixture("css-highlight");
const highlightConfig = JSON.parse(
  fs.readFileSync(path.join(highlighted, "zfb.config.json"), "utf8"),
);
const highlight = await compileCss({
  cwd: highlighted,
  input: "entry.css",
  config: highlightConfig,
});
execFileSync(
  process.env.ZFB_CSS_API_CLI,
  ["css", "--input", "entry.css", "--output", "actual.css"],
  { cwd: highlighted },
);
assert.equal(highlight.css, fs.readFileSync(path.join(highlighted, "actual.css"), "utf8"));
assert(highlight.css.includes(".hi-kw"));
const presetRoot = path.join(root, "config/node_modules/@fixture/preset");
write(presetRoot, "package.json", '{"name":"@fixture/preset","version":"1.0.0"}');
write(presetRoot, "templates/source.html", '<i class="inline-block"></i>');
const preset = definePreset("@fixture/preset", {
  wind: { sources: { roots: ["templates"] } },
  plugins: [{ name: "ignored", setup() {} }],
});
write(
  presetRoot,
  "wind.json",
  JSON.stringify({ schemaVersion: 1, specVersion: 1, producer: "fixture", candidates: ["italic"] }),
);
preset.wind.manifests = definePreset("@fixture/preset", {
  wind: { manifests: { fixture: { path: "./wind.json" } } },
}).wind.manifests;
const presetSnapshot = JSON.stringify(preset);
const composed = await compileCss({ ...options, sources: [], config: { presets: [preset] } });
assert(composed.css.includes(".inline-block"));
assert(composed.css.includes(".italic"));
const themedConfig = {
  wind: {
    reset: "minimal-v1",
    tokens: { colors: { brand: "#123456" } },
    dark: { attribute: "data-theme", value: "dark" },
    breakpoints: { wide: { minWidthPx: 900 } },
    safelist: { test: ["bg-brand", "wide:flex", "dark:bg-brand"] },
  },
  codeHighlight: { mode: "class", roleClasses: { keyword: "underline" } },
};
write(root, "invoke/zfb.config.json", JSON.stringify(themedConfig));
const themed = await compileCss({
  cwd: path.join(root, "invoke"),
  input: "input.css",
  noAutoSource: true,
  config: themedConfig,
});
execFileSync(
  process.env.ZFB_CSS_API_CLI,
  ["css", "--input", "input.css", "--output", "themed.css", "--no-auto-source"],
  { cwd: path.join(root, "invoke") },
);
assert.equal(themed.css, fs.readFileSync(path.join(root, "invoke/themed.css"), "utf8"));
for (const text of ["#123456", "900px", "data-theme", ".underline", "border-box"])
  assert(themed.css.includes(text), text);
const withoutStyles = await compileCss({
  cwd: highlighted,
  input: "entry.css",
  config: highlightConfig,
  noDefaultHighlightStyles: true,
});
assert(!withoutStyles.css.includes(".hi-kw"));
const inlineHighlight = await compileCss({
  cwd: highlighted,
  input: "entry.css",
  config: highlightConfig,
  codeHighlightMode: "inline",
});
assert(!inlineHighlight.css.includes(".hi-kw"));
assert.equal(JSON.stringify(preset), presetSnapshot);
assert.equal(process.cwd(), cwdBefore);
// Loading ordinary SDK graphs does not require a native artifact.
fs.renameSync(path.join(carrier, "zfb-css.node"), path.join(carrier, "hidden.node"));
try {
  execFileSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      'await import("@takazudo/zfb"); await import("@takazudo/zfb/runtime"); await import("@takazudo/zfb/css");',
    ],
    { cwd: room },
  );
  assert.throws(
    () =>
      execFileSync(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          'const {compileCss}=await import("@takazudo/zfb/css"); await compileCss({input:"missing.css"});',
        ],
        { cwd: room, stdio: "pipe" },
      ),
    /Cannot load the packaged CSS compiler/,
  );
} finally {
  fs.renameSync(path.join(carrier, "hidden.node"), path.join(carrier, "zfb-css.node"));
}
fs.writeFileSync(path.join(room, "browser.css"), golden.css);
console.log(
  "PASS: installed CSS API golden, CLI parity, anchors, isolation, diagnostics, atomic output, presets, lazy loading",
);

write(root, "invoke/browser.css", ".flex { display: grid }");
for (const placement of ["after-authored", "before-authored"]) {
  const result = await compileCss({
    cwd: path.join(root, "invoke"),
    input: "browser.css",
    noAutoSource: true,
    config: {
      wind: { reset: "minimal-v1", utilities: { placement }, safelist: { test: ["flex"] } },
    },
  });
  fs.writeFileSync(path.join(room, `${placement}.css`), result.css);
}
