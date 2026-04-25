// Static page at "/about". One of the acceptance fixtures.

export const meta = {
  title: "About",
  layout: "default",
};

export default function About() {
  return (
    <section className="page page-about">
      <h2>About zfb</h2>
      <p>This is the about page.</p>
    </section>
  );
}
