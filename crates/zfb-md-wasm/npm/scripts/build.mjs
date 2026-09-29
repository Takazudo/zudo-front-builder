#!/usr/bin/env node
/**
 * scripts/build.mjs — builds @takazudo/zfb-md-wasm (zfb#1577, epic zfb#1572).
 *
 * Builds four isolated wasm artifacts sequentially:
 *   - the default artifact (all default-on Cargo features -- compile,
 *     renderHtml, parseToAst, highlightCode) under `src/wasm/`, served by the
 *     package's `.` entry.
 *   - explicit highlight, render, and parse singleton feature artifacts under
 *     `src/wasm-highlight/`, `src/wasm-render/`, and `src/wasm-parse/`.
 *
 * Each artifact goes through the same 3-step pipeline:
 *   1. cargo rustc --target wasm32-unknown-unknown --profile wasm-release
 *      -p zfb-md-wasm --crate-type cdylib [--no-default-features]
 *      (the size-optimized profile added to the repo root Cargo.toml by
 *      this issue — opt-level "z", LTO, 1 codegen unit, panic=abort;
 *      opt-in via --profile so it never changes the default `release`
 *      profile other crates/binaries build with. `rustc --crate-type
 *      cdylib` because the manifest is rlib-only — see Cargo.toml)
 *   2. wasm-bindgen --target web                (ESM glue, browser + Node)
 *   3. wasm-opt -O1 (binaryen, pinned via the `binaryen` devDependency)
 *      For the parse-only and highlight-only artifacts, also derives a
 *      synchronous createGlue() factory from wasm-bindgen's canonical web
 *      glue so each Workerd instance owns its mutable JS glue state.
 *
 * Sequencing between the four artifacts is load-bearing: every cargo rustc
 * invocation writes the SAME cdylib path
 * (target/wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm) — the
 * default artifact's cdylib must be fully consumed by wasm-bindgen (step 2)
 * before the next cargo rustc pass overwrites it. The four artifacts are
 * therefore built one after the other, never in parallel.
 *
 * After all four artifacts:
 *   4. tsc (src/*.ts -> dist/*.js) — compiles every entry (index/browser/
 *      and every direct/browser singleton) in one pass, after all generated
 *      resource directories exist.
 *   5. mark each artifact's generated glue as a zfb file-loader resource,
 *      then copy all four source resource directories into `dist`.
 *
 * Prints raw, generated-glue, final, and gzip sizes for every artifact (epic
 * #1572's download-size concern; #1579's CI size-report line and #1580's docs
 * page read these numbers back out of this script's stdout).
 *
 * Usage: node scripts/build.mjs   (run via `pnpm build` / `pnpm --filter
 * @takazudo/zfb-md-wasm build`)
 */

import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  rmSync,
  cpSync,
  renameSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";
import { SHIPPED_SIZES } from "../../shipped-sizes.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const pkgRoot = resolve(__dirname, ".."); // crates/zfb-md-wasm/npm
const repoRoot = resolve(pkgRoot, "../../.."); // repo root
const crateName = "zfb-md-wasm";

// wasm-bindgen-cli is a cargo-installed tool (not an npm package like
// `binaryen` below, so it can't be pinned via package.json/pnpm-lock.yaml).
// wasm-bindgen requires an EXACT version match between this CLI and the
// `wasm-bindgen` crate resolved in the workspace Cargo.lock -- pinned here,
// following crates/zfb-toolchain-pins' "pin + verify at use-time" pattern
// used for esbuild and wrangler. Bump procedure: update this
// constant to match `grep -A1 '^name = "wasm-bindgen"$' Cargo.lock`, then
// `cargo install wasm-bindgen-cli --version <new> --locked --force`.
const EXPECTED_WASM_BINDGEN_VERSION = "0.2.121";

// wasm-opt's optimization level. -O1 was chosen empirically over -Oz/-O2/-O3:
// on this crate's output, more aggressive levels shrink the *raw* .wasm
// further but produce a *larger* gzip payload (aggressive inlining reduces
// the redundancy gzip exploits) — see this package's README "Artifact size"
// section for the measured numbers. -O1 minimized the gzip size we actually
// ship over the wire. Shared by all four artifacts.
const WASM_OPT_LEVEL = "-O1";

export const ARTIFACTS = [
  {
    label: "default",
    entry: ".",
    cargoFeatureArgs: [],
    outName: "zfb_md_wasm",
    dirName: "wasm",
    gzipCeiling: SHIPPED_SIZES.ceilings.root,
  },
  {
    label: "highlight-only",
    entry: "./highlight",
    cargoFeatureArgs: ["--no-default-features", "--features", "highlight"],
    outName: "zfb_md_wasm_highlight",
    dirName: "wasm-highlight",
    workerd: true,
    gzipCeiling: SHIPPED_SIZES.ceilings.highlight,
  },
  {
    label: "render-only",
    entry: "./render",
    cargoFeatureArgs: ["--no-default-features", "--features", "render"],
    outName: "zfb_md_wasm_render",
    dirName: "wasm-render",
    gzipCeiling: SHIPPED_SIZES.ceilings.render,
  },
  {
    label: "parse-only",
    entry: "./parse",
    cargoFeatureArgs: ["--no-default-features", "--features", "parse"],
    outName: "zfb_md_wasm_parse",
    dirName: "wasm-parse",
    workerd: true,
    gzipCeiling: SHIPPED_SIZES.ceilings.parse,
  },
];

const FACTORY_CAPABILITIES = new Set(["parseToAst", "highlightCode"]);

function matchingBrace(source, openIndex) {
  let depth = 0;
  let quote = null;
  let escaped = false;

  for (let index = openIndex; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];

    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }

    if (character === "/" && next === "/") {
      index += 2;
      while (index < source.length && source[index] !== "\n") index += 1;
      continue;
    }
    if (character === "/" && next === "*") {
      index += 2;
      while (index < source.length && !(source[index] === "*" && source[index + 1] === "/")) {
        index += 1;
      }
      index += 1;
      continue;
    }
    if (character === "'" || character === '"' || character === "`") {
      quote = character;
      continue;
    }
    if (character === "{") depth += 1;
    if (character === "}") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }

  throw new Error("wasm-bindgen factory transform found an unterminated function body");
}

function countCodeIdentifier(source, identifier) {
  let count = 0;
  let quote = null;
  let escaped = false;

  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];

    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }

    if (character === "/" && next === "/") {
      index += 2;
      while (index < source.length && source[index] !== "\n") index += 1;
      continue;
    }
    if (character === "/" && next === "*") {
      index += 2;
      while (index < source.length && !(source[index] === "*" && source[index + 1] === "/")) {
        index += 1;
      }
      index += 1;
      continue;
    }
    if (character === "'" || character === '"' || character === "`") {
      quote = character;
      continue;
    }

    if (!source.startsWith(identifier, index)) continue;
    const previous = source[index - 1];
    const following = source[index + identifier.length];
    const isIdentifierCharacter = (value) => value !== undefined && /[A-Za-z0-9_$]/.test(value);
    if (!isIdentifierCharacter(previous) && !isIdentifierCharacter(following)) {
      count += 1;
      index += identifier.length - 1;
    }
  }

  return count;
}

function removeFunction(source, name) {
  const pattern = new RegExp(
    `(?:^|\\n)[ \\t]*(?:async[ \\t]+)?function[ \\t]+${name}[ \\t]*\\(`,
    "g",
  );
  const matches = [...source.matchAll(pattern)];
  if (matches.length !== 1) {
    throw new Error(
      `wasm-bindgen factory transform expected one function declaration for ${name}; found ${matches.length}`,
    );
  }

  const match = matches[0];
  const functionIndex = match.index + match[0].lastIndexOf("function");
  const declarationStart = source.lastIndexOf("\n", match.index) + 1;
  const parameterEnd = source.indexOf(")", functionIndex);
  const bodyStart = source.indexOf("{", parameterEnd + 1);
  if (bodyStart === -1) {
    throw new Error(`wasm-bindgen factory transform could not find the body for ${name}`);
  }
  const bodyEnd = matchingBrace(source, bodyStart);
  return source.slice(0, declarationStart) + source.slice(bodyEnd + 1);
}

function parseFactoryExports(source, capability) {
  const exportPattern = /^[ \t]*export[ \t]*\{([\s\S]*?)\}[ \t]*;?[ \t]*$/gm;
  const matches = [...source.matchAll(exportPattern)];
  const expectedBlock = ["initSync", "__wbg_init as default"].sort();
  const expectedFunctionExports = ["__forceTrapForTests", capability, "version"].sort();
  if (matches.length !== 1) {
    throw new Error(
      `wasm-bindgen factory transform expected one generated export block; found ${matches.length}`,
    );
  }

  const actualBlock = matches[0][1]
    .split(",")
    .map((name) => name.trim().replace(/\s+/g, " "))
    .filter(Boolean)
    .sort();
  const functionExportPattern =
    /^[ \t]*export[ \t]+(?:async[ \t]+)?function[ \t]+([A-Za-z_$][\w$]*)[ \t]*\(/gm;
  const actualFunctionExports = [...source.matchAll(functionExportPattern)]
    .map((match) => match[1])
    .sort();
  if (
    JSON.stringify(actualBlock) !== JSON.stringify(expectedBlock) ||
    JSON.stringify(actualFunctionExports) !== JSON.stringify(expectedFunctionExports)
  ) {
    throw new Error(
      `wasm-bindgen factory transform expected exports [` +
        `${[...expectedFunctionExports, ...expectedBlock].sort().join(", ")}]; ` +
        `received [` +
        `${[...actualFunctionExports, ...actualBlock].sort().join(", ")}]`,
    );
  }
  return { exportBlock: matches[0] };
}

function factoryDeclaration(capability) {
  const parameter = capability === "parseToAst" ? "source" : "code";
  return [
    "export function createGlue(): {",
    "  initSync(input: { module: WebAssembly.Module }): unknown;",
    `  ${capability}(${parameter}: string, optionsJson: string): string;`,
    "  version(): string;",
    "  __forceTrapForTests(): void;",
    "};",
    "",
  ].join("\n");
}

/**
 * Convert pinned wasm-bindgen web-target glue into a synchronous, per-instance
 * factory. The generated module body is enclosed by createGlue(), so all
 * wasm-bindgen state (including cached views, externrefs, and finalizers) is
 * local to one instance. The strict export/token checks intentionally fail
 * closed when wasm-bindgen changes its output shape.
 */
export function transformWasmBindgenGlueToFactory(source, capability) {
  if (!FACTORY_CAPABILITIES.has(capability)) {
    throw new Error(`unsupported wasm-bindgen factory capability: ${capability}`);
  }

  const { exportBlock } = parseFactoryExports(source, capability);
  let body =
    source.slice(0, exportBlock.index) + source.slice(exportBlock.index + exportBlock[0].length);
  body = body.replace(/^([ \t]*)export([ \t]+(?:async[ \t]+)?function[ \t]+)/gm, "$1$2");
  for (const name of ["__wbg_load", "__wbg_init"]) {
    body = removeFunction(body, name);
  }

  for (const name of ["initSync", capability, "version", "__forceTrapForTests"]) {
    const declaration = new RegExp(
      `(?:^|\\n)[ \\t]*(?:async[ \\t]+)?function[ \\t]+${name}[ \\t]*\\(`,
    );
    if (!declaration.test(body)) {
      throw new Error(`wasm-bindgen factory transform expected function declaration ${name}`);
    }
  }

  const factorySource =
    `export function createGlue() {\n${body.trim()}\n` +
    `  return { initSync, ${capability}, version, __forceTrapForTests };\n}\n`;
  const forbidden = [
    ["import.meta", /\bimport\s*\.\s*meta\b/],
    ["fetch(", /\bfetch\s*\(/],
    ["import(", /\bimport\s*\(/],
    ["__wbg_init", /\b__wbg_init\b/],
    ["__wbg_load", /\b__wbg_load\b/],
    [".zfb-resource.mjs", /\.zfb-resource\.mjs/],
  ];
  for (const [token, pattern] of forbidden) {
    if (pattern.test(factorySource)) {
      throw new Error(`wasm-bindgen factory transform left forbidden token ${token}`);
    }
  }

  if (countCodeIdentifier(factorySource, "export") !== 1) {
    throw new Error("wasm-bindgen factory transform left a residual export");
  }
  if (!/^export function createGlue\(\)/m.test(factorySource)) {
    throw new Error("wasm-bindgen factory transform did not emit createGlue as its only export");
  }
  if (source.includes("__wbindgen_start") && !factorySource.includes("__wbindgen_start")) {
    throw new Error("wasm-bindgen factory transform removed synchronous __wbindgen_start");
  }

  return { javascript: factorySource, declaration: factoryDeclaration(capability) };
}

function log(msg) {
  console.log(`[build] ${msg}`);
}

function run(cmd, args, opts = {}) {
  log(`+ ${cmd} ${args.join(" ")}`);
  execFileSync(cmd, args, { stdio: "inherit", cwd: repoRoot, ...opts });
}

// Local-machine quirk (not a CI concern, see crates/zfb-md-wasm/SPIKE-FINDINGS.md
// "Local-machine quirk"): on some Macs, Homebrew's rustc shadows the
// rustup-managed toolchain on PATH even under `rustup run`, breaking wasm
// target resolution. Prepending the active rustup toolchain's own bin dir
// (when present) fixes this without affecting CI. Ask rustup which toolchain
// is active; directory enumeration can select an older installed toolchain.
function envWithRustupPathFix() {
  const env = { ...process.env };
  try {
    const rustcPath = execFileSync("rustup", ["which", "rustc"], {
      encoding: "utf8",
      env,
    }).trim();
    const binDir = dirname(rustcPath);
    if (existsSync(resolve(binDir, "rustc"))) {
      env.PATH = `${binDir}:${env.PATH ?? ""}`;
    }
  } catch {
    // A plain rustc installation remains usable without rustup.
  }
  return env;
}

function checkWasmBindgenVersion(env) {
  let out;
  try {
    out = execFileSync("wasm-bindgen", ["--version"], { encoding: "utf8", env });
  } catch {
    throw new Error(
      `wasm-bindgen CLI not found on PATH. Install with:\n` +
        `  cargo install wasm-bindgen-cli --version ${EXPECTED_WASM_BINDGEN_VERSION} --locked`,
    );
  }
  const found = out.trim().split(/\s+/).pop();
  if (found !== EXPECTED_WASM_BINDGEN_VERSION) {
    throw new Error(
      `wasm-bindgen CLI version mismatch: found ${found}, expected ${EXPECTED_WASM_BINDGEN_VERSION} ` +
        `(must exactly match the wasm-bindgen crate version in the repo root Cargo.lock, or wasm-bindgen ` +
        `will refuse to process the .wasm file). Reinstall:\n` +
        `  cargo install wasm-bindgen-cli --version ${EXPECTED_WASM_BINDGEN_VERSION} --locked --force`,
    );
  }
  log(`wasm-bindgen ${found} OK`);
}

function cargoTargetDirectory(env) {
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: repoRoot,
      encoding: "utf8",
      env,
    }),
  );
  if (typeof metadata.target_directory !== "string") {
    throw new Error("cargo metadata did not report a target_directory");
  }
  return metadata.target_directory;
}

function resolveWasmOptBin() {
  const bin = resolve(pkgRoot, "node_modules/.bin/wasm-opt");
  if (!existsSync(bin)) {
    throw new Error(
      `wasm-opt not found at ${bin} — run \`pnpm install\` first (binaryen devDependency).`,
    );
  }
  return bin;
}

function fmtBytes(n) {
  return `${n.toLocaleString("en-US")} bytes (${(n / 1024 / 1024).toFixed(2)} MB)`;
}

// The local Mac/CI codegen-drift escape hatch (zfb#3054, moved from an env
// var to a CLI flag by zfb#3060/#3057): on some Macs the render-only
// artifact's gzip-9 size lands a few KB over its ceiling while CI's build of
// the same source is under it, which otherwise stops `pnpm test:md-wasm`
// before it ever reaches the test half on a Mac. `--allow-over-ceiling`
// downgrades ceiling breaches to loud warnings -- but only outside CI.
// Inside CI it is refused outright (a hard error naming the flag) so it can
// never be used to force a PR gate green; CI's own build is, and stays, the
// authoritative oracle.
//
// The opt-in MUST be a CLI argument, not env-detected: inside this package's
// `"prepublishOnly": "pnpm build"`, the nested `pnpm build` invocation sees
// `npm_lifecycle_event=build` (pnpm rewrites it per script -- probed for
// zfb#3059), so a publish can't be distinguished from an ordinary build from
// inside this script, and any ambient env var would soften a publish it was
// never aimed at. `argv` is supplied by the caller rather than read here, so
// the policy is a pure function of what was actually passed on the command
// line (see the CLI entry at the bottom of this file).
export function ceilingPolicyFromArgv(argv, env) {
  return {
    allowOver: argv.includes("--allow-over-ceiling"),
    ci: typeof env.CI === "string" && env.CI !== "",
  };
}

// ZFB_MD_WASM_ALLOW_OVER_CEILING is the retired env-var form of the opt-in
// above (zfb#3054) -- no longer honored. Returns a warning string when a
// stale export is still set, so a developer's old shell config doesn't
// silently do nothing; null when unset.
export function legacyEnvVarWarning(env) {
  const value = env.ZFB_MD_WASM_ALLOW_OVER_CEILING;
  if (typeof value !== "string" || value === "") return null;
  return (
    "ZFB_MD_WASM_ALLOW_OVER_CEILING is no longer honored and has no effect on this build -- " +
    "the opt-in is now the --allow-over-ceiling build argument. Use `pnpm test:md-wasm:local`, " +
    "which passes it for you, or unset the stale variable."
  );
}

// Collect-then-fail: every over-ceiling artifact is reported in one pass,
// never just the first one found.
export function evaluateCeilings(stats, policy) {
  if (policy.allowOver && policy.ci) {
    return {
      errors: [
        "--allow-over-ceiling is refused when CI is set -- it exists for local Mac/CI codegen " +
          "drift only and must never be used to force a PR gate green. Omit it, or run the " +
          "strict `pnpm test:md-wasm` lane.",
      ],
      warnings: [],
    };
  }

  const errors = [];
  const warnings = [];
  for (const { label, gzipSize, gzipCeiling } of stats) {
    if (gzipSize <= gzipCeiling) continue;
    const overBy = gzipSize - gzipCeiling;
    if (policy.allowOver) {
      warnings.push(
        `${label} gzip-9 size ${gzipSize} exceeds ceiling ${gzipCeiling} (over by ${overBy} ` +
          `bytes) -- allowed locally via --allow-over-ceiling; CI remains the oracle.`,
      );
    } else {
      errors.push(`${label} gzip-9 size ${gzipSize} exceeds ceiling ${gzipCeiling}`);
    }
  }
  return { errors, warnings };
}

/**
 * Prints the complete four-artifact summary first, then any warnings, then
 * throws once with every collected error -- so a Mac codegen-drift breach on
 * one artifact never hides the other three artifacts' numbers.
 */
export function reportCeilings(stats, policy, log) {
  log("");
  log("== zfb-md-wasm build summary ==");
  for (const {
    label,
    entry,
    cdylibSize,
    bindgenSize,
    glueSize,
    glueGzipSize,
    finalSize,
    gzipSize,
  } of stats) {
    log(`-- ${label} artifact (\`${entry}\` entry) --`);
    log(`raw cdylib:                            ${fmtBytes(cdylibSize)}`);
    log(`wasm-bindgen binary:                   ${fmtBytes(bindgenSize)}`);
    log(`generated glue:                        ${fmtBytes(glueSize)}`);
    log(`generated glue gzip -9:                ${fmtBytes(glueGzipSize)}`);
    log(`wasm-opt ${WASM_OPT_LEVEL} (final):              ${fmtBytes(finalSize)}`);
    log(`gzip -9 (final):                       ${fmtBytes(gzipSize)}`);
  }

  const { errors, warnings } = evaluateCeilings(stats, policy);
  for (const warning of warnings) {
    log(`WARNING: ${warning}`);
  }
  if (errors.length > 0) {
    throw new Error(errors.join("\n"));
  }
}

/**
 * Builds one wasm artifact end-to-end (cargo rustc -> wasm-bindgen ->
 * wasm-opt) into `srcOutDir`. `workerd` selects whether this artifact also
 * emits the parse/highlight factory pair. `cargoFeatureArgs` is `[]` for the default
 * artifact or the exact singleton feature arguments for a slim artifact;
 * `outName` becomes both the wasm-bindgen `--out-name` and the emitted file
 * stems (`<outName>_bg.wasm`, `<outName>_glue.zfb-resource.mjs`, …).
 */
function buildWasmArtifact({ env, label, cargoFeatureArgs, outName, srcOutDir, workerd }) {
  rmSync(srcOutDir, { recursive: true, force: true });
  mkdirSync(srcOutDir, { recursive: true });

  // `cargo rustc --crate-type cdylib`, not `cargo build`: the crate's manifest
  // declares `crate-type = ["rlib"]` (see Cargo.toml for why — a native cdylib
  // links V8 and fails as an ELF `-shared` object). The wasm cdylib is forced
  // here for the wasm32 target only, where nothing pulls V8 into the graph.
  log(
    `== ${label} 1/3: cargo rustc --target wasm32-unknown-unknown --profile wasm-release -p ${crateName} --crate-type cdylib ${cargoFeatureArgs.join(" ")} ==`,
  );
  run(
    "cargo",
    [
      "rustc",
      "--target",
      "wasm32-unknown-unknown",
      "--profile",
      "wasm-release",
      "-p",
      crateName,
      "--crate-type",
      "cdylib",
      ...cargoFeatureArgs,
    ],
    {
      env,
    },
  );
  // Every artifact's cargo rustc pass writes this SAME path (see module doc
  // "Sequencing" note) -- read it immediately, before the next artifact's
  // cargo rustc pass (if any) overwrites it.
  const cdylibPath = resolve(
    cargoTargetDirectory(env),
    "wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm",
  );
  const cdylibSize = readFileSync(cdylibPath).length;
  log(`${label} cdylib (pre wasm-bindgen): ${fmtBytes(cdylibSize)}`);

  log(`== ${label} 2/3: wasm-bindgen --target web ==`);
  run(
    "wasm-bindgen",
    ["--target", "web", "--out-dir", srcOutDir, "--out-name", outName, cdylibPath],
    {
      env,
    },
  );
  const bgWasmPath = resolve(srcOutDir, `${outName}_bg.wasm`);
  const gluePath = resolve(srcOutDir, `${outName}.js`);
  const glueDeclarationPath = resolve(srcOutDir, `${outName}.d.ts`);
  const resourceGluePath = resolve(srcOutDir, `${outName}_glue.zfb-resource.mjs`);
  const resourceGlueDeclarationPath = resolve(srcOutDir, `${outName}_glue.zfb-resource.d.mts`);

  // One canonical generated runtime serves both variants of this entry. The
  // marker turns the browser variant's static import into an esbuild file
  // resource, while the direct variant dynamically imports this same module.
  renameSync(gluePath, resourceGluePath);
  renameSync(glueDeclarationPath, resourceGlueDeclarationPath);
  const bindgenSize = readFileSync(bgWasmPath).length;
  const glueBytes = readFileSync(resourceGluePath);
  const glueSize = glueBytes.length;
  const glueGzipSize = gzipSync(glueBytes, { level: 9 }).length;
  log(`${label} wasm-bindgen output: ${fmtBytes(bindgenSize)}`);
  log(`${label} generated glue: ${fmtBytes(glueSize)}`);
  log(`${label} generated glue gzip -9: ${fmtBytes(glueGzipSize)}`);

  log(`== ${label} 3/3: wasm-opt ${WASM_OPT_LEVEL} ==`);
  const wasmOptBin = resolveWasmOptBin();
  const optTmpPath = `${bgWasmPath}.opt`;
  run(wasmOptBin, [
    WASM_OPT_LEVEL,
    // The rustc wasm32-unknown-unknown target emits bulk-memory / sign-ext /
    // mutable-globals / nontrapping-float-to-int instructions by default;
    // wasm-opt's validator needs these named explicitly (deliberately NOT
    // --all-features, which also accepts proposals this build never emits).
    "--enable-bulk-memory",
    "--enable-sign-ext",
    "--enable-mutable-globals",
    "--enable-nontrapping-float-to-int",
    "--strip-debug",
    "--strip-producers",
    "-o",
    optTmpPath,
    bgWasmPath,
  ]);
  renameSync(optTmpPath, bgWasmPath);
  const finalSize = readFileSync(bgWasmPath).length;
  const gzipSize = gzipSync(readFileSync(bgWasmPath), { level: 9 }).length;
  log(`${label} wasm-opt output: ${fmtBytes(finalSize)}`);
  log(`${label} gzip -9: ${fmtBytes(gzipSize)}`);

  if (workerd) {
    const capability = label === "parse-only" ? "parseToAst" : "highlightCode";
    const { javascript, declaration } = transformWasmBindgenGlueToFactory(
      readFileSync(resourceGluePath, "utf8"),
      capability,
    );
    writeFileSync(resolve(srcOutDir, `${outName}_glue.zfb-factory.mjs`), javascript);
    writeFileSync(resolve(srcOutDir, `${outName}_glue.zfb-factory.d.mts`), declaration);
  }

  const expectedFiles = [
    `${outName}_glue.zfb-resource.mjs`,
    `${outName}_glue.zfb-resource.d.mts`,
    `${outName}_bg.wasm`,
    `${outName}_bg.wasm.d.ts`,
    ...(workerd ? [`${outName}_glue.zfb-factory.mjs`, `${outName}_glue.zfb-factory.d.mts`] : []),
  ].sort();
  const actualFiles = readdirSync(srcOutDir).sort();
  if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
    throw new Error(
      `${label} generated resource set is not closed: expected ${expectedFiles.join(", ")}; ` +
        `received ${actualFiles.join(", ")}`,
    );
  }

  return { cdylibSize, bindgenSize, glueSize, glueGzipSize, finalSize, gzipSize };
}

function main(argv) {
  const env = envWithRustupPathFix();

  // Emitted before the build regardless of outcome -- including when
  // nothing ends up over ceiling -- so a stale exported variable is never
  // silently inert.
  const legacyWarning = legacyEnvVarWarning(env);
  if (legacyWarning) log(`WARNING: ${legacyWarning}`);

  // Refuse the local opt-in before the four-artifact build, not after it: the
  // CI refusal in `evaluateCeilings` is a hard error either way, and reaching
  // it via `reportCeilings` would first burn the full ~15-minute build.
  const policy = ceilingPolicyFromArgv(argv, env);
  const refusal = evaluateCeilings([], policy).errors;
  if (refusal.length > 0) throw new Error(refusal.join("\n"));
  checkWasmBindgenVersion(env);

  const distDir = resolve(pkgRoot, "dist");
  rmSync(distDir, { recursive: true, force: true });

  const stats = ARTIFACTS.map((artifact) => ({
    artifact,
    stats: buildWasmArtifact({
      env,
      label: artifact.label,
      cargoFeatureArgs: artifact.cargoFeatureArgs,
      outName: artifact.outName,
      srcOutDir: resolve(pkgRoot, "src", artifact.dirName),
      workerd: artifact.workerd === true,
    }),
  }));

  log(`== tsc ==`);
  run(resolve(pkgRoot, "node_modules/.bin/tsc"), [], { cwd: pkgRoot, env });

  for (const { dirName } of ARTIFACTS) {
    const distSubDir = resolve(distDir, dirName);
    mkdirSync(distSubDir, { recursive: true });
    cpSync(resolve(pkgRoot, "src", dirName), distSubDir, { recursive: true });
  }

  const records = stats.map(({ artifact, stats: artifactStats }) => ({
    label: artifact.label,
    entry: artifact.entry,
    gzipCeiling: artifact.gzipCeiling,
    ...artifactStats,
  }));
  reportCeilings(records, policy, (msg) => console.log(msg));
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) {
  // process.argv is read at this CLI entry only, and nowhere else in the
  // module. Note that scripts/run-zfb-md-wasm-build-timed.mjs reaches this
  // branch deliberately -- it rewrites process.argv[1] to this file's path
  // before importing it -- so that wrapper's own trailing arguments are what
  // main() sees, and passing it --allow-over-ceiling does opt in. That is the
  // intended pass-through for a timed local Mac run; CI still refuses the flag.
  main(process.argv.slice(2));
}
