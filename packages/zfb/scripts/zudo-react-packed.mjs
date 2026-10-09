import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolvePackageTsc } from "../../../scripts/package-tsc.mjs";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const command = process.argv[2];

function run(binary, args, cwd) {
  execFileSync(binary, args, { cwd, stdio: "pipe", encoding: "utf8" });
}

function buildAndPack() {
  run("pnpm", ["--filter", "@takazudo/zfb-slugify", "build"], repoDir);
  run("pnpm", ["--filter", "@takazudo/zfb", "build"], repoDir);
  console.log("build: PASS");
  const packDir = mkdtempSync(join(tmpdir(), "zudo-react-pack-"));
  run("pnpm", ["pack", "--pack-destination", packDir], resolve(repoDir, "packages/zfb-slugify"));
  run("pnpm", ["pack", "--pack-destination", packDir], packageDir);
  const tarballs = readdirSync(packDir);
  const tarball = tarballs.find((name) => /^takazudo-zfb-\d/.test(name) && name.endsWith(".tgz"));
  const slugifyTarball = tarballs.find(
    (name) => name.includes("zfb-slugify") && name.endsWith(".tgz"),
  );
  if (!tarball || !slugifyTarball) throw new Error("pnpm pack did not create both tarballs");
  console.log("pack: PASS");
  return {
    zfb: join(packDir, tarball),
    "zfb-slugify": join(packDir, slugifyTarball),
  };
}

function stage(tarballs, directory) {
  for (const [name, tarball] of Object.entries(tarballs)) {
    const target = join(directory, "node_modules", "@takazudo", name);
    mkdirSync(target, { recursive: true });
    run("tar", ["-xzf", tarball, "-C", target, "--strip-components=1"], repoDir);
  }
  console.log("stage: PASS");
}

function assertPackedExports(directory) {
  const packageRoot = join(directory, "node_modules", "@takazudo", "zfb");
  const packageJson = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
  const expected = {
    "./zudo-react": ["./dist/zudo-react/index.d.ts", "./dist/zudo-react/index.js"],
    "./zudo-react/jsx-runtime": [
      "./dist/zudo-react/jsx-runtime.d.ts",
      "./dist/zudo-react/jsx-runtime.js",
    ],
    "./zudo-react/jsx-dev-runtime": [
      "./dist/zudo-react/jsx-dev-runtime.d.ts",
      "./dist/zudo-react/jsx-dev-runtime.js",
    ],
    "./zudo-react/server": ["./dist/zudo-react/server.d.ts", "./dist/zudo-react/server.js"],
    "./zudo-react/client": ["./dist/zudo-react/client.d.ts", "./dist/zudo-react/client.js"],
    "./zudo-react/testing": ["./dist/zudo-react/testing.d.ts", "./dist/zudo-react/testing.js"],
  };
  const actualSubpaths = Object.keys(packageJson.exports ?? {})
    .filter((subpath) => /^\.\/zudo-react(?:\/.*)?$/.test(subpath))
    .sort();
  if (JSON.stringify(actualSubpaths) !== JSON.stringify(Object.keys(expected).sort())) {
    throw new Error(
      `Packed package has unexpected zudo-react exports: ${actualSubpaths.join(", ")}`,
    );
  }

  for (const [subpath, [types, runtime]] of Object.entries(expected)) {
    const entry = packageJson.exports[subpath];
    if (entry?.types !== types || entry?.default !== runtime) {
      throw new Error(`Packed ${subpath} must point to ${types} and ${runtime}`);
    }
    if (!existsSync(join(packageRoot, types)) || !existsSync(join(packageRoot, runtime))) {
      throw new Error(`Packed ${subpath} has a missing declaration or runtime target`);
    }
  }

  const probe = join(directory, "server-import-probe.mjs");
  writeFileSync(
    probe,
    `if (typeof globalThis.document !== "undefined") throw new Error("unexpected DOM global");\nconst server = await import("@takazudo/zfb/zudo-react/server");\nif (Object.keys(server).sort().join(",") !== "islandRoot,renderToString,serializeProps") throw new Error("unexpected server entry exports");\nif (typeof globalThis.document !== "undefined") throw new Error("server entry touched the DOM");\n`,
  );
  run(process.execPath, [probe], directory);
  const slugProbe = join(directory, "slugify-import-probe.mjs");
  const parityFixture = resolve(repoDir, "crates/zfb-content/tests/fixtures/slugify-parity.json");
  writeFileSync(
    slugProbe,
    `import { readFileSync } from "node:fs";
import { slugify as rootSlugify, SlugAllocator as RootAllocator } from "@takazudo/zfb";
import { slugify as subSlugify, SlugAllocator as SubAllocator } from "@takazudo/zfb/slugify";
import { slugify as helperSlugify, SlugAllocator as HelperAllocator } from "@takazudo/zfb-slugify";
const fixture = JSON.parse(readFileSync(${JSON.stringify(parityFixture)}, "utf8"));
if (rootSlugify !== helperSlugify || subSlugify !== helperSlugify || RootAllocator !== HelperAllocator || SubAllocator !== HelperAllocator) throw new Error("packed compatibility identity mismatch");
for (const { input, expected } of fixture.slugify) for (const fn of [rootSlugify, subSlugify]) if (fn(input) !== expected) throw new Error("packed slugify parity mismatch");
for (const { strategy, headings, expected } of fixture.allocate) for (const Allocator of [RootAllocator, SubAllocator]) { const allocator = new Allocator(strategy); const actual = headings.map(({ depth, text }) => allocator.allocate(depth, rootSlugify(text))); if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("packed allocator parity mismatch"); }
`,
  );
  run(process.execPath, [slugProbe], directory);
  console.log("packed root/subpath/helper slugify parity: PASS");
  console.log("packed exports and DOM-free server import: PASS");
}

function writeConsumer(directory) {
  const fixtures = readdirSync(join(packageDir, "type-tests-zudo-react")).filter((name) =>
    name.endsWith(".tsx"),
  );
  for (const fixture of fixtures) {
    cpSync(join(packageDir, "type-tests-zudo-react", fixture), join(directory, fixture));
  }
  const slugifyFixture = "slugify-types.ts";
  writeFileSync(
    join(directory, slugifyFixture),
    `import { slugify as rootSlugify, SlugAllocator as RootAllocator } from "@takazudo/zfb";
import { slugify as subSlugify, SlugAllocator as SubAllocator } from "@takazudo/zfb/slugify";
import { slugify as helperSlugify, SlugAllocator as HelperAllocator } from "@takazudo/zfb-slugify";
const value: string = rootSlugify(subSlugify(helperSlugify("Hello")));
const allocators: Array<RootAllocator | SubAllocator | HelperAllocator> = [new RootAllocator(), new SubAllocator(), new HelperAllocator()];
for (const allocator of allocators) allocator.allocate(2, value);
`,
  );
  const config = JSON.parse(
    readFileSync(join(packageDir, "tsconfig.zudo-react-fixture.json"), "utf8"),
  );
  config.include = [...fixtures, slugifyFixture];
  // The repo fixture loads the workspace's @types/node; an isolated packed consumer has none.
  config.compilerOptions.types = [];
  writeFileSync(join(directory, "tsconfig.json"), JSON.stringify(config));
  config.compilerOptions.jsx = "react-jsxdev";
  writeFileSync(join(directory, "tsconfig.dev.json"), JSON.stringify(config));
}

function checkTypes(directory) {
  // The package's own compiler, plus the TS 6.0 and TS 5.9 consumers the published declarations
  // keep supporting while contributor builds use TypeScript 7 (#3544).
  const compilers = [
    resolvePackageTsc(packageDir),
    resolvePackageTsc(packageDir, "typescript-6.0"),
    resolvePackageTsc(packageDir, "typescript-5.9"),
  ];
  writeConsumer(directory);
  for (const tsc of compilers) {
    run(tsc.command, [...tsc.args, "-p", "tsconfig.json"], directory);
    console.log(`packed types production (tsc ${tsc.version}): PASS`);
    run(tsc.command, [...tsc.args, "-p", "tsconfig.dev.json"], directory);
    console.log(`packed types development (tsc ${tsc.version}): PASS`);
  }
}

function esbuildProbe(directory, kind, esbuild) {
  const entry = join(directory, "probe.tsx");
  writeFileSync(
    entry,
    'import { h } from "@takazudo/zfb/zudo-react"; export const view = <div>{h("b", null, "ok")}</div>;\n',
  );
  for (const platform of ["neutral", "browser"]) {
    const args = [
      entry,
      "--bundle",
      `--platform=${platform}`,
      "--format=esm",
      "--jsx=automatic",
      "--jsx-import-source=@takazudo/zfb/zudo-react",
      `--outfile=${join(directory, `probe-${platform}.js`)}`,
    ];
    if (kind === "source") args.push("--preserve-symlinks");
    run(esbuild, args, directory);
  }
  console.log(`esbuild probe ${kind} neutral/browser: PASS`);
}

try {
  if (command !== "stage" && command !== "check")
    throw new Error("Usage: zudo-react-packed.mjs stage <dir> | check");
  if (command === "stage" && !process.argv[3]) throw new Error("stage needs a directory");
  const tarballs = buildAndPack();
  const directory =
    command === "stage"
      ? resolve(process.argv[3])
      : mkdtempSync(join(tmpdir(), "zudo-react-consumer-"));
  stage(tarballs, directory);
  if (command === "check") {
    assertPackedExports(directory);
    checkTypes(directory);
    const sourceDir = mkdtempSync(join(tmpdir(), "zudo-react-source-"));
    const link = join(sourceDir, "node_modules", "@takazudo", "zfb");
    mkdirSync(dirname(link), { recursive: true });
    symlinkSync(packageDir, link, "dir");
    symlinkSync(
      resolve(repoDir, "packages/zfb-slugify"),
      join(sourceDir, "node_modules", "@takazudo", "zfb-slugify"),
      "dir",
    );
    const esbuild =
      process.env.ZFB_ESBUILD_BIN ||
      join(repoDir, "crates", "zfb", "binaries", "esbuild", "esbuild");
    if (existsSync(esbuild)) {
      esbuildProbe(directory, "packed", esbuild);
      esbuildProbe(sourceDir, "source", esbuild);
    } else {
      console.log("esbuild probe: SKIPPED (set ZFB_ESBUILD_BIN or stage the pinned binary)");
    }
  }
} catch (error) {
  console.error(error?.stderr?.toString() || error);
  process.exitCode = 1;
}
