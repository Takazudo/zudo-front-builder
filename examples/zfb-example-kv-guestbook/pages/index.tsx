import DefaultLayout from "../layouts/default";
import {
  MESSAGE_MAX_LENGTH,
  createGuestbookEntry,
  getGuestbookContext,
  getGuestbookKv,
  listGuestbookEntries,
  parseEntrySubmission,
  queueEntryWrite,
  type EntryListResult,
} from "../lib/kv";

export const prerender = false;

export default async function HomePage() {
  const cf = getGuestbookContext();
  if (!cf) {
    return new Response("Cloudflare request context is unavailable.", {
      status: 503,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  }

  const { request, env, ctx } = cf;
  const kv = getGuestbookKv(env);
  if (!kv) {
    return new Response("GUESTBOOK KV binding is not configured.", {
      status: 503,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  }

  if (request.method === "POST") {
    const submission = await parseEntrySubmission(request);
    if (!submission.ok) {
      return redirectTo("/", request, { error: submission.error });
    }

    const entry = createGuestbookEntry(submission.message);
    queueEntryWrite(kv, entry, ctx);
    return redirectTo("/", request, { queued: "1" });
  }

  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("Method not allowed", {
      status: 405,
      headers: {
        allow: "GET, HEAD, POST",
        "content-type": "text/plain; charset=utf-8",
      },
    });
  }

  let result: EntryListResult;
  try {
    result = await listGuestbookEntries(kv);
  } catch {
    return new Response("Guestbook entries could not be loaded.", {
      status: 503,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  }

  const url = new URL(request.url);
  const notice =
    url.searchParams.get("queued") === "1" ? "Entry queued. It may take a moment to appear." : null;
  const error = url.searchParams.get("error");

  return (
    <DefaultLayout>
      <div class="stack">
        <section>
          <h1 class="page-title">Guestbook</h1>
          <p class="muted">A small server-rendered recipe backed by Cloudflare Workers KV.</p>
        </section>

        {notice ? <div class="notice">{notice}</div> : null}
        {error ? <div class="error">{error}</div> : null}

        <section class="panel" aria-labelledby="sign-title">
          <h2 class="section-title" id="sign-title">
            Sign
          </h2>
          <form class="entry-form" method="post" action="/">
            <label for="message">Message</label>
            <textarea
              id="message"
              name="message"
              maxlength={MESSAGE_MAX_LENGTH}
              required
              rows={4}
            />
            <div class="form-row">
              <span class="muted">{MESSAGE_MAX_LENGTH} characters max</span>
              <button type="submit">Post entry</button>
            </div>
          </form>
        </section>

        <section class="panel" aria-labelledby="entries-title">
          <h2 class="section-title" id="entries-title">
            Entries
          </h2>
          {result.entries.length > 0 ? (
            <ul class="entry-list">
              {result.entries.map((entry) => (
                <li class="entry" key={entry.key}>
                  <p>{entry.message}</p>
                  <div class="entry-meta">
                    <time dateTime={entry.createdAt}>{formatDate(entry.createdAt)}</time>
                    <code>{entry.key}</code>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p class="muted">No entries yet.</p>
          )}
        </section>

        <section class="panel" aria-labelledby="api-title">
          <h2 class="section-title" id="api-title">
            API
          </h2>
          <div class="api-list">
            <code>GET /api/entries</code>
            <code>POST /api/entries</code>
            <code>DELETE /api/entries/&lt;key&gt;</code>
          </div>
        </section>
      </div>
    </DefaultLayout>
  );
}

function redirectTo(pathname: string, request: Request, params: Record<string, string>): Response {
  const url = new URL(pathname, request.url);
  for (const [key, value] of Object.entries(params)) {
    url.searchParams.set(key, value);
  }
  return new Response(null, {
    status: 303,
    headers: {
      location: `${url.pathname}${url.search}`,
      "cache-control": "no-store",
    },
  });
}

function formatDate(iso: string): string {
  return new Intl.DateTimeFormat("en", {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "UTC",
  }).format(new Date(iso));
}
