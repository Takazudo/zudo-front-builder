import { Layout } from "../components/layout";

export default function Home() {
  return (
    <Layout title="Static home" greeting island>
      <p data-page="static">prerendered</p>
    </Layout>
  );
}
