import { Island } from "@takazudo/zfb";
import { ProbeIsland } from "../components/probe-island";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-islands-chunk-retention</title>
      </head>
      <body>
        <h1>dev-islands-chunk-retention</h1>
        <Island when="load">
          <ProbeIsland />
        </Island>
      </body>
    </html>
  );
}
