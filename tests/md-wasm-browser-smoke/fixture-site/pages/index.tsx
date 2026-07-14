import { Island } from "@takazudo/zfb";

import { MdWasmHighlighter } from "../components/md-wasm-highlighter";

/**
 * Small production-build fixture for the packed @takazudo/zfb-md-wasm browser
 * condition. The highlighter stays inside a client island so the package is
 * not imported until the user explicitly asks for it in Chromium.
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>zfb-md-wasm browser smoke fixture</title>
      </head>
      <body>
        <h1>zfb-md-wasm browser smoke fixture</h1>
        <Island when="load">
          <MdWasmHighlighter />
        </Island>
      </body>
    </html>
  );
}
