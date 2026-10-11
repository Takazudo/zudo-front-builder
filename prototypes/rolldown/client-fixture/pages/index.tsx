import { clientScript } from "@takazudo/zfb";

export default function Page() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>Standalone client script probe</title>
        <link rel="icon" href="data:," />
        <script type="module" src={clientScript("probe")} />
      </head>
      <body>
        <main>
          <div id="entry" />
          <div id="lazy" />
          <div id="worker" />
          <div id="nested" />
          <div id="worker-error" />
        </main>
      </body>
    </html>
  );
}
