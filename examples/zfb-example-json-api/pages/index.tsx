import { Island } from "@takazudo/zfb";

import ItemBrowser from "../components/item-browser";
import DefaultLayout from "../layouts/default";

export default function HomePage() {
  return (
    <DefaultLayout>
      <section class="page-heading">
        <p class="eyebrow">Cloudflare Worker SSR</p>
        <h1>Searchable JSON endpoints with a hydrated Preact client</h1>
        <p>
          The static shell ships first. The island below calls the SSR API routes for filtered
          pagination and MiniSearch ranking.
        </p>
      </section>

      <Island when="load">
        <ItemBrowser />
      </Island>
    </DefaultLayout>
  );
}
