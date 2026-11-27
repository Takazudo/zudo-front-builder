import { Layout } from "../../components/layout";

export const prerender = false;

export default function Doc({ params }: { params: { slug?: string } }) {
  return (
    <Layout title={`Doc ${params.slug ?? ""}`} greeting>
      <p data-page="doc">{params.slug}</p>
    </Layout>
  );
}
