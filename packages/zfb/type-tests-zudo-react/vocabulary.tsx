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
const tags = [<search />, <hgroup />, <menu />, <pattern />, <filter />, <marker />, <image />];
void [metadata, preload, script, video, download, ordered, svg, use, tags];

// @ts-expect-error true boolean attributes reject strings.
const badAsync: JSX.IntrinsicElements["script"] = { async: "true" };
// @ts-expect-error enumerated booleans reject numbers.
const badSpellcheck: JSX.IntrinsicElements["input"] = { spellcheck: 1 };
// @ts-expect-error arbitrary attribute names remain rejected.
const unknown: JSX.IntrinsicElements["meta"] = { unknownattribute: "x" };
void [badAsync, badSpellcheck, unknown];
