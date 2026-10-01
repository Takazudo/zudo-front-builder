#!/usr/bin/env node
// Reproducible esbuild sidecar for the production shared-islands entry shape.
// This does not read or modify zfb's private resource/stage-audit metafile.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import { gzipSync } from "node:zlib";

const root = dirname(new URL(import.meta.url).pathname);
const repo = resolve(root, "../..");
const fixtures = resolve(root, "fixtures");
const cases = {
  "no-island": [],
  "event-only": ["event-only"],
  "scalar-signal": ["scalar-signal"],
  "show-for": ["show-for"],
  model: ["model"],
  "blog-theme": ["blog-menu", "theme-toggle"],
  "json-api": ["json-api"],
  "multi-island": ["scalar-signal", "theme-toggle"],
};
const names = {
  "event-only": "EventOnly",
  "scalar-signal": "ScalarSignal",
  "show-for": "ShowFor",
  model: "Model",
  "blog-menu": "BlogMenu",
  "theme-toggle": "ThemeToggle",
  "json-api": "JsonApi",
};

function option(flag) {
  const i = process.argv.indexOf(flag);
  if (i < 0 || !process.argv[i + 1]) throw new Error(`required ${flag} VALUE`);
  return process.argv[i + 1];
}
const packageDir = resolve(option("--package"));
const esbuild = resolve(option("--esbuild"));
const out = resolve(option("--out"));
const mode = option("--mode");
if (!["workspace", "packed"].includes(mode)) throw new Error("--mode must be workspace or packed");
const pin = option("--source-sha");
if (!/^[a-f0-9]{40}$/.test(pin)) throw new Error("--source-sha must be a full git SHA");
const packageJson = JSON.parse(readFileSync(join(packageDir, "package.json"), "utf8"));
if (packageJson.name !== "@takazudo/zfb") throw new Error("--package must point to @takazudo/zfb");
const esbuildVersion = execFileSync(esbuild, ["--version"], { encoding: "utf8" }).trim();
if (esbuildVersion !== "0.25.12")
  throw new Error(`expected esbuild 0.25.12, got ${esbuildVersion}`);
if (mode === "packed" && !readFileSync(join(packageDir, "dist", "zudo-react", "client.js")))
  throw new Error("packed mode requires dist/zudo-react/client.js");

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
function gzip(bytes) {
  return gzipSync(bytes, { level: 9, mtime: 0 }).length;
}
function sourcePath(path, work) {
  const absolute = resolve(work, path);
  if (absolute.startsWith(`${work}/`)) return `<consumer>/${relative(work, absolute)}`;
  if (absolute.startsWith(`${repo}/`)) return `<repo>/${relative(repo, absolute)}`;
  if (absolute.startsWith(`${packageDir}/`)) return `<package>/${relative(packageDir, absolute)}`;
  return absolute;
}
function source() {
  return Object.keys(names).map((k) => [k, sha(readFileSync(join(fixtures, `${k}.tsx`)))]);
}
function entry(ids) {
  let s = 'import { mountIslands } from "@takazudo/zfb/runtime";\n';
  s += 'import { h } from "@takazudo/zfb/zudo-react";\n';
  s += 'import { hydrate, mount } from "@takazudo/zfb/zudo-react/client";\n';
  ids.forEach((id, i) => {
    s += `import * as i${i} from ${JSON.stringify(`./${id}.tsx`)};\n`;
  });
  s += "const manifest=Object.create(null);\n";
  ids.forEach((id, i) => {
    const n = names[id];
    s += `manifest[${JSON.stringify(n)}]={identity:{component:${JSON.stringify(n)},build:"0123456789abcdef"},mount:(props,element,mode)=>mode==="hydrate"?hydrate(h(i${i}.${n},props),element,{identity:{component:${JSON.stringify(n)},build:"0123456789abcdef"}}):mount(h(i${i}.${n},props),element,{identity:{component:${JSON.stringify(n)},build:"0123456789abcdef"}})};\n`;
  });
  return s + "mountIslands(manifest);\n";
}
function runCase(label, ids, pass, keepNames = true) {
  if (!ids.length)
    return {
      chunks: [],
      initial: { raw: 0, gzip: 0 },
      later: { raw: 0, gzip: 0 },
      shipped: { raw: 0, gzip: 0 },
      inputs: [],
    };
  const work = mkdtempSync(join(tmpdir(), "zfb-island-size-"));
  try {
    const packageLink = join(work, "node_modules", "@takazudo", "zfb");
    mkdirSync(dirname(packageLink), { recursive: true });
    symlinkSync(packageDir, packageLink, "dir");
    for (const id of ids)
      writeFileSync(join(work, `${id}.tsx`), readFileSync(join(fixtures, `${id}.tsx`)));
    writeFileSync(join(work, "entry.tsx"), entry(ids));
    const dest = join(out, label, keepNames ? `pass-${pass}` : `no-keep-names-pass-${pass}`);
    mkdirSync(dest, { recursive: true });
    const args = [
      join(work, "entry.tsx"),
      "--bundle",
      "--format=esm",
      "--splitting",
      "--tree-shaking=true",
      "--platform=browser",
      "--external:node:*",
      "--loader:.css=empty",
      "--loader:.module.css=empty",
      "--jsx=automatic",
      "--jsx-import-source=@takazudo/zfb/zudo-react",
      ...(keepNames ? ["--keep-names"] : []),
      "--minify",
      `--outdir=${dest}`,
      "--entry-names=islands",
      "--chunk-names=islands-chunk-[hash]",
      '--define:process.env.NODE_ENV="production"',
      "--define:import.meta.env.PROD=true",
      "--define:import.meta.env.DEV=false",
      `--metafile=${join(dest, "metafile.json")}`,
      "--log-level=error",
    ];
    execFileSync(esbuild, args, { cwd: work, stdio: "pipe" });
    const meta = JSON.parse(readFileSync(join(dest, "metafile.json"), "utf8"));
    const paths = readdirSync(dest)
      .filter((x) => x.endsWith(".js"))
      .sort();
    const chunks = paths.map((file) => {
      const bytes = readFileSync(join(dest, file));
      const key = Object.keys(meta.outputs).find((k) => resolve(work, k) === join(dest, file));
      if (!key) throw new Error(`metafile has no output for ${file}`);
      const output = meta.outputs[key];
      return {
        file,
        raw: bytes.length,
        gzip: gzip(bytes),
        sha256: sha(bytes),
        entry: Boolean(output.entryPoint),
        imports: output.imports.map((x) => ({ path: basename(x.path), kind: x.kind })),
        inputs: Object.entries(output.inputs).map(([path, value]) => ({
          path: sourcePath(path, work),
          bytesInOutput: value.bytesInOutput,
        })),
      };
    });
    const byName = new Map(chunks.map((x) => [x.file, x]));
    const initialNames = new Set();
    function visit(name) {
      if (initialNames.has(name)) return;
      const chunk = byName.get(name);
      if (!chunk) throw new Error(`missing emitted JS import ${name}`);
      initialNames.add(name);
      for (const imp of chunk.imports)
        if (imp.kind === "import-statement" && imp.path.endsWith(".js")) visit(imp.path);
    }
    visit("islands.js");
    const sum = (rows) => ({
      raw: rows.reduce((n, x) => n + x.raw, 0),
      gzip: rows.reduce((n, x) => n + x.gzip, 0),
    });
    const inputs = Object.entries(meta.inputs)
      .map(([path, value]) => ({ path: sourcePath(path, work), bytes: value.bytes }))
      .sort((a, b) => a.path.localeCompare(b.path));
    for (const chunk of chunks) chunk.phase = initialNames.has(chunk.file) ? "initial" : "later";
    return {
      chunks,
      initial: sum(chunks.filter((x) => x.phase === "initial")),
      later: sum(chunks.filter((x) => x.phase === "later")),
      shipped: sum(chunks),
      inputs,
    };
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

mkdirSync(out, { recursive: true });
const results = {};
for (const [label, ids] of Object.entries(cases)) {
  const first = runCase(label, ids, 1);
  const second = runCase(label, ids, 2);
  const stable = JSON.stringify(first) === JSON.stringify(second);
  if (!stable) throw new Error(`${label}: two builds differ; inspect ${join(out, label)}`);
  results[label] = first;
}
const noKeepFirst = runCase("scalar-signal", cases["scalar-signal"], 1, false);
const noKeepSecond = runCase("scalar-signal", cases["scalar-signal"], 2, false);
if (JSON.stringify(noKeepFirst) !== JSON.stringify(noKeepSecond))
  throw new Error("no-keep-names repeat differs");
if (
  mode === "packed" &&
  !results["scalar-signal"].inputs.some((input) =>
    input.path.endsWith("/dist/zudo-react/client.js"),
  )
)
  throw new Error("packed graph did not retain the built zudo-react client module");
const provenance = {
  sourceSha: pin,
  packageVersion: packageJson.version,
  packageMode: mode,
  packagePath: packageDir,
  esbuildVersion,
  esbuildBinarySha256: sha(readFileSync(esbuild)),
  nodeVersion: process.version,
  zlibVersion: process.versions.zlib,
  compression: "node:zlib gzipSync level=9 mtime=0, gzip per JS chunk",
  flags:
    "bundle esm splitting tree-shaking browser keep-names minify NODE_ENV=production import.meta.env.PROD=true import.meta.env.DEV=false jsx=automatic",
  packageJsonSha256: sha(readFileSync(join(packageDir, "package.json"))),
  pnpmLockSha256: sha(readFileSync(join(repo, "pnpm-lock.yaml"))),
  cargoLockSha256: sha(readFileSync(join(repo, "Cargo.lock"))),
  fixtureSha256: Object.fromEntries(source()),
  repeats: 2,
};
const report = {
  provenance,
  results,
  keepNamesProbe: {
    case: "scalar-signal",
    withoutKeepNames: noKeepFirst.shipped,
    deltaRaw: results["scalar-signal"].shipped.raw - noKeepFirst.shipped.raw,
    deltaGzip: results["scalar-signal"].shipped.gzip - noKeepFirst.shipped.gzip,
  },
  incremental: {
    secondIslandVersusScalar: {
      raw: results["multi-island"].shipped.raw - results["scalar-signal"].shipped.raw,
      gzip: results["multi-island"].shipped.gzip - results["scalar-signal"].shipped.gzip,
    },
  },
};
writeFileSync(join(out, "measurement.json"), JSON.stringify(report, null, 2) + "\n");
let md = `# Island size measurement: ${mode}\n\nSource ${pin}; package ${packageJson.version}; esbuild ${esbuildVersion}; Node ${process.version}; zlib ${process.versions.zlib}. gzipSync level 9, mtime 0, per JS chunk. Two identical builds per case: **PASS**.\n\n| Case | Initial raw | Initial gzip | Later raw | Later gzip | All shipped raw | All shipped gzip |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n`;
for (const [label, value] of Object.entries(results))
  md += `| ${label} | ${value.initial.raw} | ${value.initial.gzip} | ${value.later.raw} | ${value.later.gzip} | ${value.shipped.raw} | ${value.shipped.gzip} |\n`;
md +=
  "\n| Case | Chunk | Phase | Raw | Gzip | SHA-256 |\n| --- | --- | --- | ---: | ---: | --- |\n";
for (const [label, value] of Object.entries(results))
  for (const chunk of value.chunks)
    md += `| ${label} | ${chunk.file} | ${chunk.phase} | ${chunk.raw} | ${chunk.gzip} | ${chunk.sha256} |\n`;
md += `\nMulti-island incremental against scalar signal: ${report.incremental.secondIslandVersusScalar.raw} raw, ${report.incremental.secondIslandVersusScalar.gzip} gzip. Shared chunks are counted once per page.\n\nPer-chunk bytes, SHA-256, imports, and retained input bytes are in measurement.json and each case's pass-1/metafile.json. Pass-2 artifacts prove repeatability. The no-island case emits no islands JS. These are controlled current-source bundle measurements, not recovered v2/v3 historical recipe numbers.\n`;
md += `\nGlobal keep-names probe (scalar signal only): ${report.keepNamesProbe.deltaRaw} raw and ${report.keepNamesProbe.deltaGzip} gzip versus the identical flags without keep-names. This is a whole-bundle delta, not attribution to one feature.\n`;
writeFileSync(join(out, "report.md"), md);
console.log(join(out, "report.md"));
