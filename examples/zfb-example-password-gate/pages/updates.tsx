import DefaultLayout from "../layouts/default";

const updates = [
  {
    date: "2026-07-10",
    title: "Preview gate wired",
    body: "Every route now passes through the Worker before static assets are served.",
  },
  {
    date: "2026-07-09",
    title: "Billing review state",
    body: "Team upgrade prompts now use shorter labels and clearer renewal dates.",
  },
  {
    date: "2026-07-08",
    title: "Invite copy pass",
    body: "External collaborator invites now explain access limits before the send action.",
  },
];

export default function UpdatesPage() {
  return (
    <DefaultLayout title="Updates - Preview Desk" active="updates">
      <section class="page-heading">
        <p class="eyebrow">Change log</p>
        <h1>Preview updates</h1>
        <p>Short notes for reviewers who need to compare this static build with earlier packets.</p>
      </section>

      <section class="timeline" aria-label="Preview updates">
        {updates.map((update) => (
          <article class="timeline-item" key={update.title}>
            <time dateTime={update.date}>{update.date}</time>
            <div>
              <h2>{update.title}</h2>
              <p>{update.body}</p>
            </div>
          </article>
        ))}
      </section>
    </DefaultLayout>
  );
}
