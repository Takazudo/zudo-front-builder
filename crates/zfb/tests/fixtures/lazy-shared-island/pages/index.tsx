import { Island } from "@takazudo/zfb";
import { LazyHost } from "../components/lazy-host";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Lazy shared island</title>
      </head>
      <body>
        <main>
          <strong>eager and lazy code share one module</strong>
        </main>
        <Island when="load">
          <LazyHost label="shared" />
        </Island>
      </body>
    </html>
  );
}
