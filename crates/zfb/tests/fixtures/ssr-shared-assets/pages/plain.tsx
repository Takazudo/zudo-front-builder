import { Layout } from "../components/layout";

export const prerender = false;

export default function Plain() {
  return (
    <Layout title="Plain page">
      <p data-page="plain">no script, no island</p>
    </Layout>
  );
}
