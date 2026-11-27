import { htmlPage } from "@takazudo/zfb-runtime/server";
import { Layout } from "../components/layout";

export const prerender = false;

export default function Missing() {
  const headers = new Headers({
    "Cache-Control": "no-store",
    Vary: "Accept-Language",
    "X-Fixture": "missing",
  });
  headers.append("Set-Cookie", "first=1; Path=/");
  headers.append("Set-Cookie", "second=2; Path=/");
  return htmlPage(
    <Layout title="Not found">
      <p data-page="missing">nothing here</p>
    </Layout>,
    { status: 404, headers },
  );
}
