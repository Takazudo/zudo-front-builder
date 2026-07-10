import DefaultLayout from "../layouts/default";

export default function NotFoundPage() {
  return (
    <DefaultLayout title="Not found">
      <section class="not-found">
        <p class="eyebrow">404</p>
        <h1>Not found</h1>
        <p>The static 404 page handles asset misses while JSON API errors keep their JSON body.</p>
        <a class="text-link" href="/">
          Back to the API console
        </a>
      </section>
    </DefaultLayout>
  );
}
