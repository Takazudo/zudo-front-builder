// Dev-only Rolldown harness for this package's tests and browser benchmark
// (#4634). It mirrors zfb's native Rolldown file-resource contract
// (crates/zfb-bundler/src/native.rs, `GraphPlugin`): a resource
// import (`?url` suffix allowed) is emitted as an asset file and becomes a
// relative `./<file>` URL string resolved by the consumer against
// import.meta.url.

import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { basename, isAbsolute, join, resolve } from "node:path";

import { rolldown } from "rolldown";

const URL_SUFFIX = "?url";

function stripUrlSuffix(id) {
  return id.endsWith(URL_SUFFIX) ? id.slice(0, -URL_SUFFIX.length) : id;
}

/** Emits each matching import as an asset and exports its relative URL string. */
export function fileResourcePlugin(extensions) {
  return {
    name: "zfb-test-file-resources",
    load(id) {
      const file = stripUrlSuffix(id);
      if (!extensions.some((extension) => file.endsWith(extension))) return null;
      const reference = this.emitFile({
        type: "asset",
        name: basename(file),
        originalFileName: id,
        source: readFileSync(file),
      });
      return {
        code: `export default import.meta.ROLLDOWN_FILE_URL_${reference};`,
        moduleType: "js",
      };
    },
    resolveFileUrl({ relativePath }) {
      return JSON.stringify(`./${relativePath}`);
    },
  };
}

/**
 * Keeps each matching import as a relative external `./<file>` import and
 * copies the source file next to the bundle.
 * `copied` collects the resolved source paths.
 */
export function copyResourcePlugin(extensions, outDir, copied) {
  return {
    name: "zfb-test-copy-resources",
    async resolveId(source, importer, options) {
      if (!extensions.some((extension) => source.endsWith(extension))) return null;
      const resolved = await this.resolve(source, importer, { ...options, skipSelf: true });
      const file = resolved?.id ?? (importer ? resolve(importer, "..", source) : source);
      if (!isAbsolute(file)) throw new Error(`cannot resolve copied resource ${source}`);
      copied.push(file);
      mkdirSync(outDir, { recursive: true });
      copyFileSync(file, join(outDir, basename(file)));
      return { id: `./${basename(file)}`, external: true };
    },
  };
}

/**
 * Bundles `input` to `outDir` as ESM and returns the bundled module ids
 * together with the output.
 * Unresolved imports and other warnings fail the bundle instead of becoming
 * silent externals.
 */
export async function bundle({
  input,
  outDir,
  platform = "browser",
  conditionNames,
  assetFileNames,
  plugins = [],
  cwd,
}) {
  const warnings = [];
  const build = await rolldown({
    input,
    cwd,
    platform,
    plugins,
    ...(conditionNames ? { resolve: { conditionNames } } : {}),
    onLog(level, log) {
      if (level === "warn") warnings.push(`${log.code ?? "WARN"}: ${log.message}`);
    },
  });
  try {
    const output = await build.write({
      dir: outDir,
      format: "esm",
      entryFileNames: "[name].js",
      ...(assetFileNames ? { assetFileNames } : {}),
    });
    if (warnings.length > 0) throw new Error(`rolldown warnings:\n${warnings.join("\n")}`);
    const inputs = output.output.flatMap((item) => (item.type === "chunk" ? item.moduleIds : []));
    return { inputs, output: output.output };
  } finally {
    await build.close();
  }
}

const COMPILED_MODULE_ID = "zfb-test:compiled-module.js";

/**
 * Bundles one in-memory ESM module to CommonJS with every import left as an
 * external `require(...)`, so a caller can evaluate it with its own `require`.
 */
export async function bundleToCommonJs(code) {
  const warnings = [];
  const build = await rolldown({
    input: COMPILED_MODULE_ID,
    platform: "neutral",
    external: (source) => source !== COMPILED_MODULE_ID,
    plugins: [
      {
        name: "zfb-test-compiled-module",
        resolveId(source) {
          return source === COMPILED_MODULE_ID ? source : null;
        },
        load(id) {
          return id === COMPILED_MODULE_ID ? { code, moduleType: "js" } : null;
        },
      },
    ],
    onLog(level, log) {
      if (level === "warn") warnings.push(`${log.code ?? "WARN"}: ${log.message}`);
    },
  });
  try {
    const { output } = await build.generate({ format: "cjs", exports: "named" });
    if (warnings.length > 0) throw new Error(`rolldown warnings:\n${warnings.join("\n")}`);
    if (output.length !== 1 || output[0].type !== "chunk") {
      throw new Error("expected exactly one CommonJS chunk");
    }
    return output[0].code;
  } finally {
    await build.close();
  }
}
