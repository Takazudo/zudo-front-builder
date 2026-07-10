import DefaultLayout from "../layouts/default";

export default function NotFoundPage() {
  return (
    <DefaultLayout title="Not found | zfb KV guestbook">
      <div class="panel">
        <h1 class="page-title">Not found</h1>
        <p class="muted">The requested page does not exist.</p>
        <p>
          <a href="/">Return to the guestbook</a>
        </p>
      </div>
    </DefaultLayout>
  );
}
