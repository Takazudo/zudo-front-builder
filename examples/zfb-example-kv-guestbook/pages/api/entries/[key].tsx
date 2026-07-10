import { requireAdmin } from "../../../lib/auth";
import {
  getGuestbookContext,
  getGuestbookKv,
  isEntryKey,
  jsonResponse,
  methodNotAllowed,
  serviceUnavailable,
} from "../../../lib/kv";

export const prerender = false;

type Props = {
  params?: {
    key?: string;
  };
};

export default async function EntryAdminApi({ params }: Props) {
  const cf = getGuestbookContext();
  if (!cf) {
    return serviceUnavailable(
      "Cloudflare request context is unavailable. Run through wrangler dev or deploy the Cloudflare adapter.",
    );
  }

  const { request, env } = cf;
  if (request.method !== "DELETE") {
    return methodNotAllowed(["DELETE"]);
  }

  const auth = await requireAdmin(request, env);
  if (!auth.ok) {
    return auth.response;
  }

  const kv = getGuestbookKv(env);
  if (!kv) {
    return serviceUnavailable("GUESTBOOK KV binding is not configured.");
  }

  const key = params?.key;
  if (!key || !isEntryKey(key)) {
    return jsonResponse({ ok: false, error: "Entry key is missing or invalid." }, { status: 400 });
  }

  try {
    await kv.delete(key);
  } catch (error) {
    console.error(
      JSON.stringify({
        message: "guestbook KV delete failed",
        key,
        error: error instanceof Error ? error.message : String(error),
      }),
    );
    return jsonResponse({ ok: false, error: "Entry could not be deleted." }, { status: 500 });
  }

  return jsonResponse({ ok: true, deleted: key });
}
