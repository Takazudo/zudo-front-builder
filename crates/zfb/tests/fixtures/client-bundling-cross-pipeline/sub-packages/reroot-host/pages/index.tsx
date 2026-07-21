import { clientScript } from "@takazudo/zfb";
import { RerootIsland } from "../src/RerootIsland";

export default function RerootHostPage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>ZFB_REROOT_HOST_PAGE</title>
        <script type="module" src={clientScript("reroot")} />
      </head>
      <body>
        <main>
          <h1>ZFB_REROOT_HOST_PAGE</h1>
          <RerootIsland />
        </main>
      </body>
    </html>
  );
}
