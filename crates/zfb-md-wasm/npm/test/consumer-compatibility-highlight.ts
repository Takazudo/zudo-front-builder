// Consumer-facing declaration fixture for the `./highlight` subpath
// (zfb#1849, epic zfb#1845). Mirrors consumer-compatibility.ts's role for
// the default entry: it imports only what `./highlight` is meant to expose,
// so `pnpm typecheck:consumer` catches an accidental `compile`/`renderHtml`
// leak into the highlight-only surface as a type error rather than at
// runtime.
import {
  highlightCode,
  init,
  version,
  type HighlightCodeOptions,
  type HighlightCodeResult,
} from "../dist/highlight.js";

async function consumeHighlightApi(): Promise<void> {
  const options: HighlightCodeOptions = { language: "javascript" };
  const result: HighlightCodeResult = await highlightCode("const x = 1;", options);

  await init();
  const packageVersion: string = await version();
  void result;
  void packageVersion;
}

void consumeHighlightApi;
