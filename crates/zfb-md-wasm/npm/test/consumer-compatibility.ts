// Frozen existing-consumer declaration fixture. It deliberately imports only
// the root exports/types that existed before `highlightCode`, so adding the
// new API cannot accidentally alter the package's established surface.
import {
  compile,
  init,
  renderHtml,
  version,
  type CompileResult,
  type Diagnostic,
  type RenderHtmlResult,
  type ZfbMdWasmOptions,
} from "../dist/index.js";

async function consumeExistingRootApi(): Promise<void> {
  const options: ZfbMdWasmOptions = {
    filename: "post.mdx",
    jsxRuntime: "preact",
    development: false,
    pipeline: { gfm: { table: true } },
  };
  const compiled: CompileResult = await compile("# title\n", options);
  const rendered: RenderHtmlResult = await renderHtml("# title\n", options);
  const diagnostic: Diagnostic | undefined = compiled.diagnostics[0] ?? rendered.diagnostics[0];
  const knownSource: Diagnostic["source"] | undefined = diagnostic?.source;

  await init();
  const packageVersion: string = await version();
  void knownSource;
  void packageVersion;
}

void consumeExistingRootApi;
