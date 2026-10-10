import { Island } from "zfb";
import { Counter } from "../components/Counter";
import Hero from "../components/Hero.mdx";
import styles from "./page.module.css";
import { label } from "@fixture/message";
export default function Page() {
  return (
    <html>
      <head>
        <title>Rolldown prototype</title>
      </head>
      <body>
        <main class={styles.hero}>
          <Hero />
          <p>{label}</p>
          <Island when="load">
            <Counter />
          </Island>
        </main>
      </body>
    </html>
  );
}
