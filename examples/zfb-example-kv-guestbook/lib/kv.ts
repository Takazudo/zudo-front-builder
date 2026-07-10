import { getCloudflareContext, type CloudflareContext } from "@takazudo/zfb-adapter-cloudflare";

export const ENTRY_PREFIX = "entry:";
export const MESSAGE_MAX_LENGTH = 240;
export const ENTRY_TTL_SECONDS = 60 * 60 * 24 * 90;
export const LIST_WINDOW_LIMIT = 40;
export const READ_FANOUT_LIMIT = 20;
export const READ_CONCURRENCY = 6;

export type EntryKey = `entry:${string}:${string}`;

export interface GuestbookEntry {
  key: EntryKey;
  message: string;
  createdAt: string;
}

export interface Env {
  GUESTBOOK?: KVNamespace<EntryKey>;
  ADMIN_TOKEN?: string;
}

export interface EntryListResult {
  entries: GuestbookEntry[];
  listedKeys: number;
  listComplete: boolean;
  fanoutLimit: number;
}

type WaitUntilContext = {
  waitUntil(promise: Promise<unknown>): void;
};

type ValidationResult =
  | { ok: true; message: string }
  | { ok: false; error: string; field: "message" };

export function getGuestbookContext(): CloudflareContext<Env> | null {
  try {
    return getCloudflareContext<Env>();
  } catch {
    return null;
  }
}

export function getGuestbookKv(env: Env): KVNamespace<EntryKey> | undefined {
  return env.GUESTBOOK;
}

export function isEntryKey(value: string): value is EntryKey {
  return value.startsWith(ENTRY_PREFIX);
}

export function jsonResponse(data: unknown, init: ResponseInit = {}): Response {
  const headers = new Headers(init.headers);
  if (!headers.has("content-type")) {
    headers.set("content-type", "application/json; charset=utf-8");
  }
  headers.set("cache-control", "no-store");
  return new Response(JSON.stringify(data, null, 2), { ...init, headers });
}

export function serviceUnavailable(message: string): Response {
  return jsonResponse({ ok: false, error: message }, { status: 503 });
}

export function methodNotAllowed(allowed: readonly string[]): Response {
  return jsonResponse(
    { ok: false, error: "Method not allowed", allowed },
    {
      status: 405,
      headers: {
        allow: allowed.join(", "),
      },
    },
  );
}

export async function parseEntrySubmission(request: Request): Promise<ValidationResult> {
  const contentType = request.headers.get("content-type")?.toLowerCase() ?? "";

  if (contentType.includes("application/json")) {
    let body: unknown;
    try {
      body = await request.json();
    } catch {
      return { ok: false, field: "message", error: "Request body must be valid JSON." };
    }
    if (!isRecord(body) || typeof body.message !== "string") {
      return { ok: false, field: "message", error: "JSON body must include a message string." };
    }
    return validateMessage(body.message);
  }

  if (
    contentType.includes("application/x-www-form-urlencoded") ||
    contentType.includes("multipart/form-data")
  ) {
    let form: FormData;
    try {
      form = await request.formData();
    } catch {
      return { ok: false, field: "message", error: "Form body could not be parsed." };
    }
    const value = form.get("message");
    return validateMessage(typeof value === "string" ? value : "");
  }

  return validateMessage(await request.text());
}

export function validateMessage(raw: string): ValidationResult {
  const message = raw.trim();
  if (message.length === 0) {
    return { ok: false, field: "message", error: "Message is required." };
  }
  if (message.length > MESSAGE_MAX_LENGTH) {
    return {
      ok: false,
      field: "message",
      error: `Message must be ${MESSAGE_MAX_LENGTH} characters or fewer.`,
    };
  }
  return { ok: true, message };
}

export function createGuestbookEntry(message: string): GuestbookEntry {
  const createdAt = new Date().toISOString();
  const key = `${ENTRY_PREFIX}${createdAt}:${randomHex(8)}` as EntryKey;
  return { key, message, createdAt };
}

export function queueEntryWrite(
  kv: KVNamespace<EntryKey>,
  entry: GuestbookEntry,
  ctx: WaitUntilContext,
): void {
  const write = kv
    .put(entry.key, JSON.stringify(entry), {
      expirationTtl: ENTRY_TTL_SECONDS,
    })
    .catch((error: unknown) => {
      console.error(
        JSON.stringify({
          message: "guestbook KV write failed",
          key: entry.key,
          error: errorMessage(error),
        }),
      );
    });

  ctx.waitUntil(write);
}

export async function listGuestbookEntries(kv: KVNamespace<EntryKey>): Promise<EntryListResult> {
  const listed = await kv.list({ prefix: ENTRY_PREFIX, limit: LIST_WINDOW_LIMIT });
  const keys = listed.keys
    .map((key) => key.name)
    .filter(isEntryKey)
    .sort((a, b) => b.localeCompare(a))
    .slice(0, READ_FANOUT_LIMIT);

  const entries: GuestbookEntry[] = [];
  for (let index = 0; index < keys.length; index += READ_CONCURRENCY) {
    const batch = keys.slice(index, index + READ_CONCURRENCY);
    const batchEntries = await Promise.all(batch.map((key) => readEntry(kv, key)));
    for (const entry of batchEntries) {
      if (entry) entries.push(entry);
    }
  }

  entries.sort((a, b) => b.createdAt.localeCompare(a.createdAt));

  return {
    entries,
    listedKeys: listed.keys.length,
    listComplete: listed.list_complete,
    fanoutLimit: READ_FANOUT_LIMIT,
  };
}

function randomHex(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function readEntry(kv: KVNamespace<EntryKey>, key: EntryKey): Promise<GuestbookEntry | null> {
  try {
    const entry = await kv.get<GuestbookEntry>(key, "json");
    if (!entry || !isGuestbookEntry(entry) || entry.key !== key) {
      return null;
    }
    return entry;
  } catch (error) {
    console.error(
      JSON.stringify({
        message: "guestbook KV read failed",
        key,
        error: errorMessage(error),
      }),
    );
    return null;
  }
}

function isGuestbookEntry(value: unknown): value is GuestbookEntry {
  if (!isRecord(value)) return false;
  return (
    typeof value.key === "string" &&
    isEntryKey(value.key) &&
    typeof value.message === "string" &&
    typeof value.createdAt === "string"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
