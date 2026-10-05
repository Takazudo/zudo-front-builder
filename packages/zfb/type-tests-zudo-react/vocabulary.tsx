import type { JSX } from "@takazudo/zfb/zudo-react/jsx-runtime";

const metadata: JSX.IntrinsicElements["meta"] = { property: "og:title", itemprop: "name" };
const preload: JSX.IntrinsicElements["link"] = {
  hreflang: "en",
  as: "font",
  integrity: "sha256-x",
};
const script: JSX.IntrinsicElements["script"] = { async: true, defer: false, nonce: "x" };
const video: JSX.IntrinsicElements["video"] = { preload: "metadata", playsinline: true };
const download: JSX.IntrinsicElements["a"] = { download: true, spellcheck: false };
const ordered: JSX.IntrinsicElements["ol"] = { start: 3, reversed: true };
const svg: JSX.IntrinsicElements["svg"] = {
  xmlns: "http://www.w3.org/2000/svg",
  focusable: "false",
};
const use: JSX.IntrinsicElements["use"] = {
  href: "#shape",
  "xlink:href": "#shape",
  "fill-opacity": 0.5,
  "stroke-dasharray": "1 2",
  "text-anchor": "middle",
};
const tags = [
  <search />,
  <hgroup />,
  <menu />,
  <pattern />,
  <filter />,
  <marker />,
  <image />,
  <ruby>
    <rb>base</rb>
    <rt>reading</rt>
  </ruby>,
];

export function StandardMarkupPage() {
  return (
    <html>
      <head>
        <meta property="og:title" content="Standard markup" />
        <link rel="preload" href="/assets/body.woff2" as="font" integrity="sha256-x" />
        <script src="/assets/site.js" defer nonce="page-nonce" />
      </head>
      <body>
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
          <use href="#shape" xlink:href="#shape" />
        </svg>
      </body>
    </html>
  );
}

void [metadata, preload, script, video, download, ordered, svg, use, tags, StandardMarkupPage];

// @ts-expect-error true boolean attributes reject strings.
const badAsync: JSX.IntrinsicElements["script"] = { async: "true" };
// @ts-expect-error enumerated booleans reject numbers.
const badSpellcheck: JSX.IntrinsicElements["input"] = { spellcheck: 1 };
// @ts-expect-error arbitrary attribute names remain rejected.
const unknown: JSX.IntrinsicElements["meta"] = { unknownattribute: "x" };
// @ts-expect-error unknown standard element names remain outside the finite vocabulary.
const unknownTag = <madeuptag />;
void [badAsync, badSpellcheck, unknown, unknownTag];
