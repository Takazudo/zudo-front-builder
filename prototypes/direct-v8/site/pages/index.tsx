import { Island } from "@takazudo/zfb";

import { Counter } from "../components/counter";
import styles from "./index.module.css";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Direct V8 fixture</title>
      </head>
      <body>
        <main class={styles.frame} data-v8-fixture="home">
          <h1>Direct V8 fixture</h1>
          <p data-v8-fixture="css-module" class={styles.marker}>
            CSS module marker
          </p>
          <Island when="load">
            <Counter />
          </Island>
          <a href="/posts/hello">Read the MDX post</a>
          <a href="/wasm">Read the Wasm answer</a>
        </main>
      </body>
    </html>
  );
}
