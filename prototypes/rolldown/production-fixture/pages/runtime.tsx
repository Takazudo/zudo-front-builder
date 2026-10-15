import { answer, legacy } from "@fixture/data";
import Card from "../components/RuntimeCard.mdx";
import styles from "./runtime.module.css";
import { answer as glueAnswer } from "../components/runtime-glue.zfb-resource.mjs";
export const prerender = false;
export default function RuntimePage() {
  return (
    <html>
      <body>
        <div class={styles.hero}>
          SSR_ONLY_318:{answer()}:CJS:{legacy}
        </div>
        <Card />
        <span id="glue">GLUE_318:{glueAnswer()}</span>
      </body>
    </html>
  );
}
