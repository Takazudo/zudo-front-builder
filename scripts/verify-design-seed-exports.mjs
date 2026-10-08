#!/usr/bin/env node
/**
 * Manager-owned L3 compiler/export confirmation for issue #3990.
 * Run from a clean implementation checkout with its freshly built binary:
 *
 * ZFB_BINARY="$PWD/target/debug/zfb" \
 * ZFB_BINARY_SOURCE_SHA="$(git rev-parse HEAD)" \
 * ZFB_BINARY_PROVENANCE="guarded cargo build -p zfb --bin zfb at this SHA" \
 * bash "$HOME/.codex/scripts/heavy-guard.sh" -- node scripts/verify-design-seed-exports.mjs
 *
 * No npm install, old installed CLI, network fetch or browser is used here.
 * ZFB_ESBUILD_BIN is honored by the supplied binary when needed.
 * ZFB_DESIGN_EXPORT_ARTIFACT_DIR optionally selects an empty evidence directory
 * outside the checkout; otherwise artifacts remain in a fresh /tmp directory.
 */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  PRESETS,
  createState,
  getSeedFiles,
  setValue,
  variableMap,
  windObject,
  zipData,
} from "../docs/src/components/playground/design-workshop/model.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** Independent CRC implementation checks ZIP data, rather than trusting its writer. */
function archiveCRC(bytes) {
  let remainder = -1;
  for (const byte of bytes) {
    remainder ^= byte;
    for (let bit = 0; bit < 8; bit++)
      remainder = (remainder >>> 1) ^ (remainder & 1 ? 0xedb88320 : 0);
  }
  return (remainder ^ -1) >>> 0;
}

export function verifyArchive(bytes, files) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const end = bytes.length - 22;
  assert.equal(view.getUint32(end, true), 0x06054b50, "ZIP end signature");
  assert.equal(view.getUint16(end + 8, true), Object.keys(files).length);
  assert.equal(view.getUint16(end + 10, true), Object.keys(files).length);
  assert.equal(view.getUint16(end + 20, true), 0);
  let central = view.getUint32(end + 16, true),
    localCursor = 0;
  const start = central,
    names = [];
  for (const expectedName of Object.keys(files)) {
    assert.equal(view.getUint32(central, true), 0x02014b50, "ZIP central signature");
    const length = view.getUint16(central + 28, true);
    const name = new TextDecoder().decode(bytes.subarray(central + 46, central + 46 + length));
    assert.equal(name, expectedName);
    const local = view.getUint32(central + 42, true);
    assert.equal(local, localCursor, "contiguous local records");
    assert.equal(view.getUint32(local, true), 0x04034b50);
    assert.equal(view.getUint16(local + 6, true), 0x0800, "UTF-8 filenames");
    assert.equal(view.getUint16(local + 8, true), 0, "stored entries");
    const size = view.getUint32(local + 18, true);
    assert.equal(view.getUint32(local + 22, true), size);
    assert.equal(view.getUint32(central + 20, true), size);
    assert.equal(view.getUint32(central + 24, true), size);
    assert.equal(view.getUint16(local + 26, true), length);
    assert.equal(view.getUint16(local + 28, true), 0);
    assert.equal(new TextDecoder().decode(bytes.subarray(local + 30, local + 30 + length)), name);
    const data = bytes.subarray(local + 30 + length, local + 30 + length + size);
    assert.deepEqual(data, new TextEncoder().encode(files[name]), `${name}: exact exported bytes`);
    assert.equal(view.getUint32(local + 14, true), archiveCRC(data), `${name}: local CRC`);
    assert.equal(view.getUint32(central + 16, true), archiveCRC(data), `${name}: central CRC`);
    names.push(name);
    localCursor += 30 + length + size;
    central += 46 + length;
  }
  assert.equal(localCursor, start);
  assert.equal(central, end);
  assert.equal(view.getUint32(end + 12, true), end - start);
  return names;
}

export function expectedCandidates(files, config) {
  const usage = [...files["usage.tsx"].matchAll(/\bclass="([^"]+)"/g)].flatMap((match) =>
    match[1].split(/\s+/),
  );
  assert.ok(usage.length, "actual exported usage has complete class literals");
  const tokens = config.tokens;
  const probe = [
    ...Object.keys(tokens.colors).map((name) => `bg-${name}`),
    ...Object.keys(tokens.spacing).map((name) =>
      name.startsWith("hsp-") ? `px-${name}` : `gap-y-${name}`,
    ),
    ...Object.keys(tokens.sizes).map((name) => `max-w-${name}`),
    ...Object.keys(tokens.fontSizes).map((name) => `text-${name}`),
    ...Object.keys(tokens.fontFamilies).map((name) => `font-${name}`),
    ...Object.keys(tokens.fontWeights).map((name) => `font-${name}`),
    ...Object.keys(tokens.radii).map((name) => `rounded-${name}`),
    "wide:bg-accent",
  ];
  return [...new Set([...usage, ...probe])];
}

const canonicalValue = (text) =>
  text
    .replace(/[\s"']/g, "")
    .replace(/#([\da-f]{3})(?![\da-f])/gi, (_, hex) => "#" + [...hex].map((x) => x + x).join(""))
    .replace(/\b0\.(\d)/g, ".$1")
    .toLowerCase();
const declarationPresent = (css, property, value) => {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return [...css.matchAll(new RegExp(`(?:[;{]\\s*)${escaped}\\s*:\\s*([^;}]+)`, "g"))].some(
    (match) => canonicalValue(match[1]) === canonicalValue(value),
  );
};

/** Validate actual build artifacts against the compiler's resolved candidate data. */
export function verifyCompilerOutput({ css, html, state, candidates, audit, explanations }) {
  assert.equal(audit.schemaVersion, 1);
  assert.equal(audit.command, "audit");
  assert.equal(audit.report.outcome, "complete");
  for (const field of [
    "diagnostics",
    "unrecognizedClasses",
    "deadClasses",
    "dynamicConstructions",
    "adjacentInterpolations",
  ]) {
    assert.deepEqual(audit.report[field], [], `Wind audit ${field}`);
  }
  assert.equal(explanations.schemaVersion, 1);
  assert.equal(explanations.command, "explain");
  assert.deepEqual(
    explanations.explanations.map((item) => item.candidate),
    candidates,
  );
  const rendered = new Set(
    [...html.matchAll(/\bclass="([^"]*)"/g)].flatMap((match) => match[1].split(/\s+/)),
  );
  assert.match(html, /Give the repeated decisions a name\./, "the actual DesignExample rendered");
  for (const item of explanations.explanations) {
    assert.equal(item.outcome, "resolved_utility", `${item.candidate}: compiler resolution`);
    assert.deepEqual(item.diagnostics, [], `${item.candidate}: compiler diagnostics`);
    assert.ok(rendered.has(item.candidate), `${item.candidate}: rendered HTML class`);
    assert.ok(item.selector && css.includes(item.selector), `${item.candidate}: emitted selector`);
    const escapedSelector = item.selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const match = css.match(new RegExp(escapedSelector + "\\s*\\{([^}]*)\\}"));
    assert.ok(match, `${item.candidate}: emitted exact rule`);
    const rule = "{" + match[1] + "}";
    for (const declaration of item.declarations) {
      assert.ok(
        declarationPresent(rule, declaration.property, declaration.value),
        `${item.candidate}: ${declaration.property}: ${declaration.value}`,
      );
    }
    for (const token of item.tokenResolutions) {
      assert.ok(token.configuredValue !== null, `${item.candidate}: configured ${token.tokenName}`);
      assert.ok(
        declarationPresent(css, token.variable, token.configuredValue),
        `${item.candidate}: emitted ${token.variable} binding`,
      );
    }
  }
  for (const [name, value] of Object.entries(variableMap(state.values))) {
    assert.ok(
      declarationPresent(css, name, value),
      `${name}: emitted current owned value ${value}`,
    );
  }
  assert.match(css, /@media[^{}]*960px/, "exported wide breakpoint is compiled");
}

function command(binary, args, cwd, logPath, input) {
  const result = spawnSync(binary, args, {
    cwd,
    encoding: "utf8",
    input,
    maxBuffer: 64 * 1024 * 1024,
  });
  writeFileSync(logPath, result.stdout ?? "");
  writeFileSync(logPath + ".stderr", result.stderr ?? "");
  if (result.error || result.status !== 0)
    throw new Error(
      `${binary} ${args.join(" ")} failed (${result.status}): ${result.error?.message ?? result.stderr}\nEvidence: ${logPath}`,
    );
  return result.stdout;
}

function walk(root) {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? walk(join(root, entry.name)) : [join(root, entry.name)],
  );
}

export function verifyDesignSeedExports(env = process.env) {
  for (const name of ["ZFB_BINARY", "ZFB_BINARY_SOURCE_SHA", "ZFB_BINARY_PROVENANCE"]) {
    assert.ok(env[name]?.trim(), `Set ${name}; an installed old CLI cannot verify current exports`);
  }
  const sourceSha = command(
    "git",
    ["rev-parse", "HEAD"],
    repoRoot,
    join(tmpdir(), `zfb-design-export-sha-${process.pid}.log`),
  ).trim();
  assert.match(env.ZFB_BINARY_SOURCE_SHA, /^[\da-f]{40}$/);
  assert.equal(
    env.ZFB_BINARY_SOURCE_SHA,
    sourceSha,
    "binary source SHA must match verification checkout HEAD",
  );
  assert.equal(
    spawnSync("git", ["diff", "--quiet"], { cwd: repoRoot }).status,
    0,
    "checkout tracked files must be clean",
  );
  assert.equal(
    spawnSync("git", ["diff", "--cached", "--quiet"], { cwd: repoRoot }).status,
    0,
    "checkout index must be clean",
  );
  const binarySource = realpathSync(resolve(repoRoot, env.ZFB_BINARY));
  const artifacts = env.ZFB_DESIGN_EXPORT_ARTIFACT_DIR
    ? resolve(env.ZFB_DESIGN_EXPORT_ARTIFACT_DIR)
    : mkdtempSync(join(tmpdir(), "zfb-design-exports-"));
  mkdirSync(artifacts, { recursive: true });
  const realArtifacts = realpathSync(artifacts),
    inside = relative(repoRoot, realArtifacts);
  assert.ok(
    inside.startsWith("..") || isAbsolute(inside),
    "artifact directory must be outside checkout",
  );
  assert.deepEqual(readdirSync(realArtifacts), [], "artifact directory must start empty");
  console.log(`Design export evidence: ${realArtifacts}`);
  const binary = join(realArtifacts, "zfb-bin");
  copyFileSync(binarySource, binary);
  chmodSync(binary, 0o755);
  const metadata = {
    sourceSha,
    binarySourceSha: env.ZFB_BINARY_SOURCE_SHA,
    binaryProvenance: env.ZFB_BINARY_PROVENANCE,
    binarySource,
    binarySha256: hash(readFileSync(binary)),
    nodeVersion: process.version,
    cases: [],
    outcome: "running",
  };
  const save = () =>
    writeFileSync(join(realArtifacts, "metadata.json"), JSON.stringify(metadata, null, 2) + "\n");
  save();
  try {
    metadata.binaryVersion = command(
      binary,
      ["--version"],
      realArtifacts,
      join(realArtifacts, "binary-version.log"),
    ).trim();
    if (env.ZFB_ESBUILD_BIN)
      metadata.esbuild = {
        path: realpathSync(env.ZFB_ESBUILD_BIN),
        sha256: hash(readFileSync(env.ZFB_ESBUILD_BIN)),
      };
    for (const preset of Object.keys(PRESETS))
      for (const edited of [false, true]) {
        const state = createState(preset);
        if (edited)
          for (const [key, value] of Object.entries({
            accent: "#ef0123",
            headingFont: preset === "workbench" ? "serif" : "mono",
            bodySize: 19,
            lineHeight: 1.9,
            groupGap: 22,
            sectionGap: 60,
            horizontalSpace: 34,
            radius: 15,
            readingWidth: 70,
          }))
            assert.equal(setValue(state, key, value), true);
        const label = `${preset}${edited ? "-edited" : ""}`,
          root = join(realArtifacts, label),
          project = join(root, "project"),
          exports = join(root, "exports");
        for (const directory of [
          exports,
          join(project, "pages"),
          join(project, "components"),
          join(project, "styles"),
        ])
          mkdirSync(directory, { recursive: true });
        const files = getSeedFiles(state),
          archive = zipData(files);
        verifyArchive(archive, files);
        assert.deepEqual(zipData(getSeedFiles(state)), archive, "deterministic generator and ZIP");
        for (const [name, content] of Object.entries(files))
          writeFileSync(join(exports, name), content);
        writeFileSync(join(root, "seed.zip"), archive);
        // Copy actual exports unchanged; the harness only supplies project wiring.
        copyFileSync(join(exports, "zfb.design.ts"), join(project, "zfb.design.ts"));
        copyFileSync(join(exports, "usage.tsx"), join(project, "components", "usage.tsx"));
        copyFileSync(
          join(exports, "design-system.css"),
          join(project, "styles", "design-system.css"),
        );
        writeFileSync(
          join(project, "zfb.config.ts"),
          'import { wind } from "./zfb.design";\nexport default { outDir: "dist", wind };\n',
        );
        writeFileSync(join(project, "styles", "global.css"), '@import "./design-system.css";\n');
        const candidates = expectedCandidates(files, windObject(state.values));
        writeFileSync(
          join(project, "pages", "index.tsx"),
          `import "../styles/global.css";\nimport { DesignExample } from "../components/usage";\nexport default function Home() { return <html lang="en"><head><meta charset="utf-8"/><title>Export verification</title></head><body><DesignExample/><div hidden>${candidates.map((candidate) => `<span class="${candidate}"/>`).join("")}</div></body></html>; }\n`,
        );
        const result = {
          label,
          state,
          fileSha256: Object.fromEntries(
            Object.entries(files).map(([name, content]) => [name, hash(content)]),
          ),
          archiveSha256: hash(archive),
          outcome: "running",
        };
        metadata.cases.push(result);
        save();
        command(binary, ["build"], project, join(root, "build.log"));
        const audit = JSON.parse(
          command(
            binary,
            ["wind", "audit", "--project-root", ".", "--json", "--fail-on", "error"],
            project,
            join(root, "audit.json"),
          ),
        );
        const explanations = JSON.parse(
          command(
            binary,
            ["wind", "explain", "--project-root", ".", "--json", "--stdin"],
            project,
            join(root, "explanations.json"),
            candidates.join("\n") + "\n",
          ),
        );
        const dist = join(project, "dist"),
          html = readFileSync(join(dist, "index.html"), "utf8");
        const cssFiles = walk(dist).filter((path) => path.endsWith(".css"));
        assert.ok(cssFiles.length, "real build emitted CSS");
        const linked = [...html.matchAll(/<link\b[^>]*>/g)].filter((match) =>
          /rel="stylesheet"/.test(match[0]),
        );
        assert.ok(linked.length, "actual HTML links the compiled stylesheet");
        for (const match of linked) {
          const href = match[0].match(/href="([^"]+)"/)?.[1];
          assert.ok(href?.startsWith("/") && !href.startsWith("//"), "local generated CSS link");
          assert.ok(cssFiles.includes(join(dist, href.slice(1))), `${href}: emitted linked file`);
        }
        const linkedCssFiles = [
          ...new Set(
            linked.map((match) => join(dist, match[0].match(/href="([^"]+)"/)[1].slice(1))),
          ),
        ];
        const css = linkedCssFiles.map((path) => readFileSync(path, "utf8")).join("\n");
        verifyCompilerOutput({ css, html, state, candidates, audit, explanations });
        result.outcome = "passed";
        result.candidateCount = candidates.length;
        result.css = cssFiles.map((path) => ({
          path: relative(root, path),
          sha256: hash(readFileSync(path)),
        }));
        save();
        console.log(
          `[PASS] ${label}: 5 archive entries; ${candidates.length} compiled/rendered utility candidates; current CSS values`,
        );
      }
    metadata.outcome = "passed";
    save();
    console.log(
      `[PASS] six actual builds; source ${sourceSha}; binary SHA256 ${metadata.binarySha256}`,
    );
    return realArtifacts;
  } catch (error) {
    metadata.outcome = "failed";
    const running = metadata.cases.find((item) => item.outcome === "running");
    if (running) running.outcome = "failed";
    metadata.error = error.message;
    save();
    throw error;
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    verifyDesignSeedExports();
  } catch (error) {
    console.error(`[FAIL] ${error.stack}`);
    process.exitCode = 1;
  }
}
