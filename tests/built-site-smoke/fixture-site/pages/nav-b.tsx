import { Island } from "@takazudo/zfb";
import { ClientRouter } from "@takazudo/zfb-runtime";
import { ChangedProbe, EqualProbe } from "../components/navigation-probes";
import { persistNavigationHtml } from "../components/persist-html";

export default function NavB() {
  return persistNavigationHtml(
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>nav B</title>
        <ClientRouter />
      </head>
      <body>
        <h1>Navigation B</h1>
        <a id="to-a" href="/nav-a.html">
          To A
        </a>
        <Island when="load">
          <EqualProbe label="same" />
        </Island>
        <Island when="load">
          <ChangedProbe label="new" />
        </Island>
      </body>
    </html>,
  );
}
