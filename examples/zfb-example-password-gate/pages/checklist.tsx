import DefaultLayout from "../layouts/default";

const checklist = [
  {
    owner: "Design",
    item: "Confirm empty states for the activity timeline and saved report list.",
    status: "Ready",
  },
  {
    owner: "Product",
    item: "Review billing copy for the team upgrade prompt.",
    status: "Open",
  },
  {
    owner: "Support",
    item: "Add two canned replies for invite and password-reset questions.",
    status: "Ready",
  },
];

export default function ChecklistPage() {
  return (
    <DefaultLayout title="Checklist - Preview Desk" active="checklist">
      <section class="page-heading">
        <p class="eyebrow">Release readiness</p>
        <h1>Final preview checklist</h1>
        <p>
          The gate keeps this static packet private while stakeholders review the last open launch
          items.
        </p>
      </section>

      <section class="table-panel" aria-labelledby="checklist-title">
        <div class="section-title-row">
          <h2 id="checklist-title">Open items</h2>
          <span class="count-badge">3 items</span>
        </div>
        <div class="checklist-table">
          {checklist.map((entry) => (
            <div class="check-row" key={entry.item}>
              <span class="row-owner">{entry.owner}</span>
              <span>{entry.item}</span>
              <span class={entry.status === "Ready" ? "status ready" : "status open"}>
                {entry.status}
              </span>
            </div>
          ))}
        </div>
      </section>
    </DefaultLayout>
  );
}
