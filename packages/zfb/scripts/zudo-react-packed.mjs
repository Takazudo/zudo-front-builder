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

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const command = process.argv[2];

function run(binary, args, cwd) {
  execFileSync(binary, args, { cwd, stdio: "pipe", encoding: "utf8" });
}

function buildAndPack() {
  run("pnpm", ["--filter", "@takazudo/zfb", "build"], repoDir);
  console.log("build: PASS");
  const packDir = mkdtempSync(join(tmpdir(), "zudo-react-pack-"));
  run("pnpm", ["pack", "--pack-destination", packDir], packageDir);
  const tarball = readdirSync(packDir).find((name) => name.endsWith(".tgz"));
  if (!tarball) throw new Error("pnpm pack did not create a tarball");
  console.log("pack: PASS");
  return join(packDir, tarball);
}

function stage(tarball, directory) {
  const target = join(directory, "node_modules", "@takazudo", "zfb");
  mkdirSync(target, { recursive: true });
  run("tar", ["-xzf", tarball, "-C", target, "--strip-components=1"], repoDir);
  console.log("stage: PASS");
}

function writeConsumer(directory) {
  const source = join(packageDir, "type-tests-zudo-react", "consumer.tsx");
  cpSync(source, join(directory, "consumer.tsx"));
  const config = JSON.parse(
    readFileSync(join(packageDir, "tsconfig.zudo-react-fixture.json"), "utf8"),
  );
  config.include = ["consumer.tsx"];
  writeFileSync(join(directory, "tsconfig.json"), JSON.stringify(config));
  config.compilerOptions.jsx = "react-jsxdev";
  writeFileSync(join(directory, "tsconfig.dev.json"), JSON.stringify(config));
}

function checkTypes(directory) {
  writeConsumer(directory);
  const tsc = join(packageDir, "node_modules", "typescript", "bin", "tsc");
  run(process.execPath, [tsc, "-p", "tsconfig.json"], directory);
  console.log("packed types production: PASS");
  run(process.execPath, [tsc, "-p", "tsconfig.dev.json"], directory);
  console.log("packed types development: PASS");
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
  const tarball = buildAndPack();
  const directory =
    command === "stage"
      ? resolve(process.argv[3])
      : mkdtempSync(join(tmpdir(), "zudo-react-consumer-"));
  stage(tarball, directory);
  if (command === "check") {
    checkTypes(directory);
    const sourceDir = mkdtempSync(join(tmpdir(), "zudo-react-source-"));
    const link = join(sourceDir, "node_modules", "@takazudo", "zfb");
    mkdirSync(dirname(link), { recursive: true });
    symlinkSync(packageDir, link, "dir");
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
