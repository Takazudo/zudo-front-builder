import type { Child } from "@takazudo/zfb/zudo-react";

const INLINE_CSS = `body[data-zfb-wind="raw"] { font-family: "Wind & Raw"; --quoted-css: "<raw & trusted>"; }`;
const INLINE_SCRIPT = `window.__zfbWindRaw = "quoted <& trusted";`;

export default function WindRawLayout({ children }: { children: Child }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Wind raw HTML SSR route</title>
        <style rawHtml={INLINE_CSS} />
        <script rawHtml={INLINE_SCRIPT} />
        <script src="/wind-layout.js" />
      </head>
      <body data-zfb-wind="raw">{children}</body>
    </html>
  );
}
