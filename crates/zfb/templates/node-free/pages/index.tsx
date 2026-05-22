/**
 * Home page — single static page for the node-free template.
 *
 * Intentionally simple: no content collections, no getCollection() call.
 * Content collections in @takazudo/zfb-runtime currently require Node-only
 * `node:fs` at build time (see issue #392). Until that gap is closed, the
 * Node-free template renders entirely from inline TSX, which the embedded
 * esbuild Go binary + V8 host can build without any Node toolchain.
 */
export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>node-free · zfb</title>
      </head>
      <body>
        <h1>node-free</h1>
        <p>
          A minimal <code>zfb</code> site — no Node, no pnpm required. The <code>zfb</code> binary
          alone scaffolds, builds, and serves this page.
        </p>
        <h2>Next steps</h2>
        <ul>
          <li>
            Edit <code>pages/index.tsx</code> to change this page.
          </li>
          <li>
            Add more pages by dropping <code>.tsx</code> files into <code>pages/</code>.
          </li>
          <li>
            See the docs at{" "}
            <a href="https://github.com/Takazudo/zudo-front-builder">Takazudo/zudo-front-builder</a>
            .
          </li>
        </ul>
      </body>
    </html>
  );
}
