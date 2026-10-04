import { signal } from "@takazudo/zfb/zudo-react";

// #3621: rawHtml payloads the guard accepts, each with its exact visible text.
// The browser spec keeps an identical copy and checks the two agree.
export const CASES: readonly (readonly [string, string])[] = [
  ["data-zfb-island=&quot;Demo&quot;", 'data-zfb-island="Demo"'],
  [
    "grep -roh &#39;data-zfb-island=&quot;[^&quot;]*&quot;&#39; dist/ | sort -u",
    `grep -roh 'data-zfb-island="[^"]*"' dist/ | sort -u`,
  ],
  [
    '<span style="color:#a3be8c;">&#39;data-zfb-island=&quot;[^&quot;]*&quot;&#39;</span>',
    `'data-zfb-island="[^"]*"'`,
  ],
  ['&lt;div data-zfb-island-skip-ssr="Demo"&gt;', '<div data-zfb-island-skip-ssr="Demo">'],
  ["&lt;!--zr:1:9:h--&gt;&lt;!--/zr:1:9--&gt;", "<!--zr:1:9:h--><!--/zr:1:9-->"],
  [`<span title='data-zfb-island="Demo"'>quoted</span>`, "quoted"],
  [`<span title="<!--zr:1:9:h-->">comment-like</span>`, "comment-like"],
  [`<span title=data-zfb-island=Demo>unquoted</span>`, "unquoted"],
  ["<!-- data-zfb-island=x -->a<!---->b<!-->c<!--->d</>e", "abcde"],
];

const live = signal(CASES[0]![0]);

export default function RawHtmlMarkers() {
  return (
    <div>
      {CASES.map(([payload], index) => (
        <pre data-case={String(index)} rawHtml={payload} />
      ))}
      <pre data-case="live" rawHtml={live} />
    </div>
  );
}

export function run() {
  return { live, cases: CASES };
}
