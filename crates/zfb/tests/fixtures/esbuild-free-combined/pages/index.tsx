import { clientScript, Island } from "@takazudo/zfb";
import { stamp } from "virtual:combined-stamp";
import answerWasm from "../wasm/answer.wasm";
import rawNote from "../data/note.txt?raw";
import { Card } from "../components/card/card";
import { LazyHost } from "../components/lazy-host";

declare const __COMBINED_CONFIG_MARKER__: string;

function wasmAnswer(module: WebAssembly.Module): number {
  const exported = new WebAssembly.Instance(module).exports.answer;
  if (typeof exported !== "function") {
    throw new Error("combined Wasm fixture must export answer()");
  }
  return exported() as number;
}

export default function Home() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>esbuild-free combined</title>
        <script type="module" src={clientScript("combined")} />
      </head>
      <body>
        <main>
          <p id="page">ZFB_COMBINED_PAGE:v1</p>
          <p id="config">{__COMBINED_CONFIG_MARKER__}</p>
          <p id="plugin">{stamp}</p>
          <p id="wasm">ZFB_WASM_ANSWER:{wasmAnswer(answerWasm)}</p>
          <p id="raw">{rawNote.trim()}</p>
          <Card title="ZFB_CSS_MODULE_CARD" />
        </main>
        <Island when="load">
          <LazyHost label="ZFB_LAZY_HOST" />
        </Island>
      </body>
    </html>
  );
}
