import "../styles/global.css";
import DefaultLayout from "../layouts/default";

export default function Home() {
  return (
    <DefaultLayout>
      <main id="owned-ssr">
        <h1>Owned runtime SSR</h1>
        <p>First real build</p>
      </main>
    </DefaultLayout>
  );
}
