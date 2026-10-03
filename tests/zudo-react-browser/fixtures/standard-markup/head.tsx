export default function StandardMarkupHead() {
  return (
    <>
      <meta property="og:title" content="Standard markup integration" />
      <link rel="preload" href="/standard-head.js" as="script" />
      <script src="/standard-head.js" defer nonce="fixture-nonce" />
    </>
  );
}
