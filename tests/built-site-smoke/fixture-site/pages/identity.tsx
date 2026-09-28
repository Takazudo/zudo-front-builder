import { Island } from "@takazudo/zfb";
import { DeferredProbe, IdentityProbe, SkipSsrProbe } from "../components/identity-probes";

export default function IdentityPage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>identity proof</title>
      </head>
      <body>
        <h1>Identity proof</h1>
        <Island when="load">
          <IdentityProbe id="first" />
        </Island>
        <Island when="load">
          <IdentityProbe id="second" />
        </Island>
        <Island when="load" ssrFallback={<span id="skip-fallback">server fallback</span>}>
          <SkipSsrProbe />
        </Island>
        <Island when="media" media="(min-width: 99999px)">
          <DeferredProbe />
        </Island>
      </body>
    </html>
  );
}
