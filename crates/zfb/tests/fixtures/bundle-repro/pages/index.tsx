import { Island } from "@takazudo/zfb";
import { Counter } from "../components/counter";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Bundle reproducibility</title>
      </head>
      <body>
        <main>
          <strong>same build inputs, same emitted files</strong>
        </main>
        <Island when="load">
          <Counter />
        </Island>
      </body>
    </html>
  );
}
