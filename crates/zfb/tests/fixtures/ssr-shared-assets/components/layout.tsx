import { clientScript } from "@takazudo/zfb";
import { IslandSlot } from "./island-slot";

// Evaluated at module top level on purpose: the base prefix must already be
// set when page modules are evaluated, not only at render time.
const GREETING_SCRIPT = clientScript("greeting");

export function Layout({
  title,
  greeting = false,
  island = false,
  children,
}: {
  title: string;
  greeting?: boolean;
  island?: boolean;
  children?: unknown;
}) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>{title}</title>
        {greeting ? <script type="module" src={GREETING_SCRIPT} /> : null}
      </head>
      <body>
        <main>
          <h1 class="shared-marker">{title}</h1>
          {children}
        </main>
        {island ? <IslandSlot label={title} /> : null}
      </body>
    </html>
  );
}
