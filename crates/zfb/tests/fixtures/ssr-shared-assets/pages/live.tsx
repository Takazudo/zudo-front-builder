import { Layout } from "../components/layout";

export const prerender = false;

export default function Live() {
  return (
    <Layout title="Live page" greeting island>
      <p data-page="live">rendered per request</p>
    </Layout>
  );
}
