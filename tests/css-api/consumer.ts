import {
  compileCss,
  CssCompileError,
  type CompileCssOptions,
  type CompileCssResult,
  type CssDiagnostic,
} from "@takazudo/zfb/css";
import { definePreset, type ZfbConfig } from "@takazudo/zfb/config";
const config: ZfbConfig = { wind: { sources: { roots: ["templates"] } } };
const options: CompileCssOptions = {
  input: "entry.css",
  sources: [],
  config: { ...config, presets: [definePreset("@example/preset", config)] },
};
const result: CompileCssResult = await compileCss(options);
const diagnostic: CssDiagnostic | undefined = result.diagnostics[0];
if (diagnostic) console.log(diagnostic.origin.line, diagnostic.severity);
const error = new CssCompileError("test");
console.log(result.css, result.warnings, error.code);
