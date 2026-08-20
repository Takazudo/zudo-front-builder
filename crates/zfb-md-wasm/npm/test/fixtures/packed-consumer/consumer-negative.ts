// Every directive below must remain an error: singleton artifacts must not
// accidentally grow a sibling capability through bundler/type unification.
// @ts-expect-error render does not expose parseToAst
import { parseToAst as renderParse } from "@takazudo/zfb-md-wasm/render";
// @ts-expect-error render does not expose toMdastRoot
import { toMdastRoot as renderAdapter } from "@takazudo/zfb-md-wasm/render";
// @ts-expect-error render does not expose compile
import { compile as renderCompile } from "@takazudo/zfb-md-wasm/render";
// @ts-expect-error render does not expose highlightCode
import { highlightCode as renderHighlight } from "@takazudo/zfb-md-wasm/render";
// @ts-expect-error parse does not expose renderHtml
import { renderHtml as parseRender } from "@takazudo/zfb-md-wasm/parse";
// @ts-expect-error parse does not expose compile
import { compile as parseCompile } from "@takazudo/zfb-md-wasm/parse";
// @ts-expect-error parse does not expose highlightCode
import { highlightCode as parseHighlight } from "@takazudo/zfb-md-wasm/parse";
// @ts-expect-error parse does not expose render options
import { ZfbMdWasmOptions } from "@takazudo/zfb-md-wasm/parse";

void [
  renderParse,
  renderAdapter,
  renderCompile,
  renderHighlight,
  parseRender,
  parseCompile,
  parseHighlight,
  ZfbMdWasmOptions,
];
