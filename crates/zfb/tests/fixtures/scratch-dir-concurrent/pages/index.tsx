import { Island } from "@takazudo/zfb";
import { OriginBadge } from "../components/origin-badge";

declare const __ORIGIN__: string;

export default function Home() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>scratch-dir-concurrent fixture</title>
      </head>
      <body>
        <p>SSR-ORIGIN-{__ORIGIN__}</p>
        <Island when="load">
          <OriginBadge />
        </Island>
        <a href="/posts/alpha">Alpha</a>
      </body>
    </html>
  );
}
