import { Island } from "@takazudo/zfb";
import { Counter, SkipCounter } from "../components/counter";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Owned island fixture</title>
      </head>
      <body>
        <main>
          <strong>node-free</strong>
        </main>
        <Island when="load">
          <Counter label="ready" />
        </Island>
        <Island when="idle" ssrFallback={<p>Waiting</p>}>
          <SkipCounter label="later" />
        </Island>
      </body>
    </html>
  );
}
