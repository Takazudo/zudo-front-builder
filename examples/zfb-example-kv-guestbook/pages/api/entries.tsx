import {
  createGuestbookEntry,
  getGuestbookContext,
  getGuestbookKv,
  jsonResponse,
  listGuestbookEntries,
  methodNotAllowed,
  parseEntrySubmission,
  queueEntryWrite,
  serviceUnavailable,
} from "../../lib/kv";

export const prerender = false;

export default async function EntriesApi() {
  const cf = getGuestbookContext();
  if (!cf) {
    return serviceUnavailable(
      "Cloudflare request context is unavailable. Run through wrangler dev or deploy the Cloudflare adapter.",
    );
  }

  const { request, env, ctx } = cf;
  const kv = getGuestbookKv(env);
  if (!kv) {
    return serviceUnavailable("GUESTBOOK KV binding is not configured.");
  }

  if (request.method === "GET") {
    const result = await listGuestbookEntries(kv);
    return jsonResponse({
      ok: true,
      entries: result.entries,
      listedKeys: result.listedKeys,
      listComplete: result.listComplete,
      fanoutLimit: result.fanoutLimit,
    });
  }

  if (request.method === "POST") {
    const submission = await parseEntrySubmission(request);
    if (!submission.ok) {
      return jsonResponse({ ok: false, error: submission.error }, { status: 400 });
    }

    const entry = createGuestbookEntry(submission.message);
    queueEntryWrite(kv, entry, ctx);

    return jsonResponse(
      {
        ok: true,
        queued: true,
        entry,
        consistency:
          "Write queued with ctx.waitUntil(); KV is eventually consistent, so immediate reads may not include it.",
      },
      { status: 202 },
    );
  }

  return methodNotAllowed(["GET", "POST"]);
}
