import { Island } from "@takazudo/zfb";
import { ClientRouter } from "@takazudo/zfb-runtime";
import { ChangedProbe, DisposableProbe, EqualProbe } from "../components/navigation-probes";
import { persistNavigationHtml } from "../components/persist-html";

export default function NavA() {
  return persistNavigationHtml(
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>nav A</title>
        <ClientRouter />
      </head>
      <body>
        <h1>Navigation A</h1>
        <a id="to-b" href="/nav-b/index.html">
          To B
        </a>
        <Island when="load">
          <DisposableProbe />
        </Island>
        <Island when="load">
          <EqualProbe label="same" />
        </Island>
        <Island when="load">
          <ChangedProbe label="old" />
        </Island>
      </body>
    </html>,
  );
}
