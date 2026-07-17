import { clientScript } from "@takazudo/zfb";

export default function SiblingHostPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>ZFB_SIBLING_HOST_PAGE</title>
        <script type="module" src={clientScript("entry")} />
      </head>
      <body>
        <main>
          <h1>ZFB_SIBLING_HOST_PAGE</h1>
        </main>
      </body>
    </html>
  );
}
