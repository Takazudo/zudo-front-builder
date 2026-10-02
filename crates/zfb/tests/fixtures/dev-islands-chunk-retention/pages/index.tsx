import { Island } from "@takazudo/zfb";
import { ProbeIsland } from "../components/probe-island";
import pageNote from "../content/page-note.txt?raw";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>dev-islands-chunk-retention</title>
      </head>
      <body>
        <h1>{pageNote.trim()}</h1>
        <Island when="load">
          <ProbeIsland />
        </Island>
      </body>
    </html>
  );
}
