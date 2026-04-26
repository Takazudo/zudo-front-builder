import DefaultLayout from "../layouts/default";

export default function HomePage() {
  return (
    <DefaultLayout title="Welcome to zfb">
      <main>
        <h1>Hello from zfb</h1>
        <p>
          Welcome to your new zudo-front-builder site. Edit <code>pages/index.tsx</code> to get
          started.
        </p>
        <p>
          <a href="/blog/welcome">Read the welcome post</a>
        </p>
      </main>
    </DefaultLayout>
  );
}
