/**
 * Static page whose SSR body embeds, as ordinary rendered prose, the
 * exact substrings the pre-#1713 splice-based dev content-trace
 * wrapper text-searched the COMPILED BUNDLE for ("export default" and
 * " as default") to locate the real default-export statement it
 * needed to rewrite. A page ABOUT JS export syntax makes the compiled
 * bundle contain those substrings twice — once as the actual
 * `export { ... as default }` statement, once inside this route's
 * rendered HTML string literal — which is exactly the shape a naive
 * text search could latch onto incorrectly. The wrapper-as-module
 * design (#1713) never text-searches the bundle, so this route's
 * content is inert decoy: it must serve unchanged (#1715 confirm
 * pass).
 */
export default function JsSyntaxNotePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>js syntax note</title>
      </head>
      <body>
        <p data-testid="decoy-note">
          A JS module commonly ends with a line like <code>export default Foo;</code>. When
          re-exporting an existing binding instead, the shape is{" "}
          <code>
            export {"{"} Foo as default {"}"}
          </code>{" "}
          — the binding named Foo as default becomes the module&apos;s default export.
        </p>
      </body>
    </html>
  );
}
