import { createRequire } from "node:module";
import { isAbsolute, sep } from "node:path";
import { readdirSync } from "node:fs";
import type { CodeHighlightMode, ZfbConfig } from "./config.js";

export type CssCompileConfig = Pick<ZfbConfig, "wind" | "codeHighlight" | "outDir" | "presets">;

export interface CompileCssOptions {
  input: string;
  cwd?: string;
  projectRoot?: string;
  configDir?: string;
  config?: CssCompileConfig;
  sources?: string[];
  noAutoSource?: boolean;
  codeHighlightMode?: CodeHighlightMode;
  noDefaultHighlightStyles?: boolean;
  output?: string;
}

export interface CssDiagnosticOrigin {
  sourceId: string | null;
  path: string | null;
  line: number | null;
  /** One-based byte column. */
  column: number | null;
  byteOffset: number | null;
  label: string | null;
}

export interface CssDiagnostic {
  code: string;
  severity: "warning" | "error";
  message: string;
  candidate: string | null;
  origin: CssDiagnosticOrigin;
}

export interface CompileCssResult {
  css: string;
  diagnostics: CssDiagnostic[];
  /** Source planning and configuration warnings, preserved verbatim. */
  warnings: string[];
}

export class CssCompileError extends Error {
  readonly diagnostics: CssDiagnostic[];
  readonly code: string | undefined;
  readonly severity: "warning" | "error" | undefined;
  readonly origin: CssDiagnosticOrigin | undefined;

  constructor(message: string, diagnostics: CssDiagnostic[] = [], options?: ErrorOptions) {
    super(message, options);
    this.name = "CssCompileError";
    this.diagnostics = diagnostics;
    const primary = diagnostics.find((item) => item.severity === "error") ?? diagnostics[0];
    this.code = primary?.code;
    this.severity = primary?.severity;
    this.origin = primary?.origin;
  }
}

interface Binding {
  compileCssNative(request: string): Promise<string>;
}
let binding: Binding | undefined;
const require = createRequire(import.meta.url);

function loadBinding(): Binding {
  if (binding) return binding;
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "linux") {
    const report = process.report?.getReport() as { header?: { glibcVersionRuntime?: string } };
    let musl = false;
    try {
      musl = readdirSync("/lib").some((name) => name.startsWith("ld-musl-"));
    } catch {
      /* The report remains authoritative when /lib is inaccessible. */
    }
    if (!report.header?.glibcVersionRuntime && musl) {
      throw new CssCompileError("@takazudo/zfb/css requires glibc on Linux; musl is not supported");
    }
  }
  const carrier =
    platform === "darwin" && (arch === "arm64" || arch === "x64")
      ? `darwin-${arch}`
      : platform === "linux" && (arch === "arm64" || arch === "x64")
        ? `linux-${arch}-gnu`
        : platform === "win32" && arch === "x64"
          ? "win32-x64-msvc"
          : undefined;
  if (!carrier) throw new CssCompileError(`Unsupported CSS compiler platform: ${platform}/${arch}`);
  const name = `@takazudo/zfb-${carrier}`;
  try {
    const sdk = require("../package.json") as { version: string };
    const installed = require(`${name}/package.json`) as { version: string };
    if (installed.version !== sdk.version) {
      throw new Error(`Expected ${name}@${sdk.version}, found ${installed.version}`);
    }
    const loaded = require(`${name}/zfb-css.node`) as Binding;
    if (typeof loaded.compileCssNative !== "function")
      throw new Error("Native CSS entry is missing");
    binding = loaded;
    return loaded;
  } catch (cause) {
    throw new CssCompileError(
      `Cannot load the packaged CSS compiler from ${name}/zfb-css.node. Install the matching platform package with optional dependencies enabled.`,
      [],
      { cause },
    );
  }
}

function jsonValue(value: unknown, path: string, parents = new Set<object>()): unknown {
  if (value === null || typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value !== "object")
    throw new CssCompileError(`${path}: expected a JSON-compatible value`);
  if (parents.has(value)) throw new CssCompileError(`${path}: circular configuration`);
  if (
    !Array.isArray(value) &&
    Object.getPrototypeOf(value) !== Object.prototype &&
    Object.getPrototypeOf(value) !== null
  ) {
    throw new CssCompileError(`${path}: expected a plain object`);
  }
  parents.add(value);
  const result = Array.isArray(value)
    ? Array.from(value, (item, index) => jsonValue(item, `${path}[${index}]`, parents))
    : Object.fromEntries(
        Object.entries(value)
          .filter(([, item]) => item !== undefined)
          .map(([key, item]) => [key, jsonValue(item, `${path}.${key}`, parents)]),
      );
  parents.delete(value);
  return result;
}

function projectConfig(
  value: CssCompileConfig,
  path = "config",
  parents = new Set<object>(),
): unknown {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new CssCompileError(`${path}: expected an object`);
  if (parents.has(value)) throw new CssCompileError(`${path}: circular configuration`);
  parents.add(value);
  const projected: Record<string, unknown> = {};
  for (const key of ["wind", "codeHighlight", "outDir"] as const) {
    if (value[key] !== undefined) projected[key] = jsonValue(value[key], `${path}.${key}`);
  }
  if (value.presets !== undefined) {
    if (!Array.isArray(value.presets))
      throw new CssCompileError(`${path}.presets: expected an array`);
    projected.presets = value.presets.map((preset, index) =>
      projectConfig(preset, `${path}.presets[${index}]`, parents),
    );
  }
  parents.delete(value);
  return projected;
}

/** Compile a CSS file in-process. Configuration files are never discovered or evaluated. */
export async function compileCss(options: CompileCssOptions): Promise<CompileCssResult> {
  try {
    const invocationCwd = process.cwd();
    const cwd =
      options.cwd === undefined
        ? invocationCwd
        : isAbsolute(options.cwd)
          ? options.cwd
          : `${invocationCwd}${sep}${options.cwd}`;
    // Rust preserves symlink/parent semantics; path.resolve would collapse them here.
    const request = {
      ...options,
      cwd,
      ...(options.config === undefined ? {} : { config: projectConfig(options.config) }),
    };
    const response = JSON.parse(await loadBinding().compileCssNative(JSON.stringify(request))) as {
      result?: CompileCssResult;
      error?: { message: string; diagnostics: CssDiagnostic[] };
    };
    if (response.error)
      throw new CssCompileError(response.error.message, response.error.diagnostics);
    if (!response.result)
      throw new CssCompileError("Native CSS compiler returned an invalid response");
    return response.result;
  } catch (cause) {
    if (cause instanceof CssCompileError) throw cause;
    throw new CssCompileError(cause instanceof Error ? cause.message : String(cause), [], {
      cause,
    });
  }
}
