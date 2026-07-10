import { jsonResponse, type Env } from "./kv";

type AuthResult = { ok: true } | { ok: false; response: Response };

type TimingSafeSubtleCrypto = SubtleCrypto & {
  timingSafeEqual(a: ArrayBuffer | ArrayBufferView, b: ArrayBuffer | ArrayBufferView): boolean;
};

const encoder = new TextEncoder();

export async function requireAdmin(request: Request, env: Env): Promise<AuthResult> {
  const expected = env.ADMIN_TOKEN;
  if (typeof expected !== "string" || expected.length === 0) {
    return {
      ok: false,
      response: jsonResponse(
        { ok: false, error: "ADMIN_TOKEN is not configured." },
        { status: 503 },
      ),
    };
  }

  const provided = bearerToken(request.headers.get("authorization"));
  if (!provided || !(await timingSafeEqual(provided, expected))) {
    return {
      ok: false,
      response: jsonResponse({ ok: false, error: "Unauthorized." }, { status: 401 }),
    };
  }

  return { ok: true };
}

function bearerToken(authorization: string | null): string | null {
  if (!authorization) return null;
  const match = authorization.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || null;
}

async function timingSafeEqual(provided: string, expected: string): Promise<boolean> {
  const [providedHash, expectedHash] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(provided)),
    crypto.subtle.digest("SHA-256", encoder.encode(expected)),
  ]);

  return (crypto.subtle as TimingSafeSubtleCrypto).timingSafeEqual(providedHash, expectedHash);
}
