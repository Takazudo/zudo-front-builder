import { Island } from "@takazudo/zfb";
import { ProbeIsland } from "../components/probe-island";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>dev-islands-entry-probe</title>
      </head>
      <body>
        <h1>dev-islands-entry-probe</h1>
        <Island when="load">
          <ProbeIsland />
        </Island>
      </body>
    </html>
  );
}
