import { renderToString } from "@takazudo/zfb/zudo-react/server";
import type { Description } from "@takazudo/zfb/zudo-react";

/** Keep the scanner-visible Island JSX; add the router's marker to its emitted wrapper. */
export function persistNavigationHtml(page: Description): string {
  let html = renderToString(page);
  for (const [component, id] of [
    ["EqualProbe", "equal"],
    ["ChangedProbe", "changed"],
  ]) {
    const marker = `<div data-zfb-island="${component}"`;
    if (!html.includes(marker)) throw new Error(`missing owned wrapper for ${component}`);
    html = html.replace(
      marker,
      `<div data-zfb-transition-persist="${id}" data-zfb-island="${component}"`,
    );
  }
  return html;
}
