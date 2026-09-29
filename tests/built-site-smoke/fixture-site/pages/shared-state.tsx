import { Island } from "@takazudo/zfb";
import { SharedReader } from "../components/shared-reader";
import { SharedWriter } from "../components/shared-writer";

export default function SharedStatePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>shared runtime</title>
      </head>
      <body>
        <h1>Shared runtime</h1>
        <Island when="load">
          <SharedWriter />
        </Island>
        <Island when="load">
          <SharedReader />
        </Island>
      </body>
    </html>
  );
}
