// Static page at "/". Hello-world to anchor the routing tree.

export const meta = {
  title: "Home",
  layout: "default",
};

export default function Index() {
  return (
    <section class="page page-index">
      <p>Welcome to zfb.</p>
    </section>
  );
}
