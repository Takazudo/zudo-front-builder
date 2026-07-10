import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";

import { summarizeText, type AiEnv, type SummaryResult } from "../../lib/ai";

export const prerender = false;

type ErrorBody = {
  error: string;
};

export default async function SummarizeApi(request: Request): Promise<Response> {
  if (request.method !== "POST") {
    return json<ErrorBody>({ error: "Use POST with a JSON body." }, 405);
  }

  const body = await readJson(request);
  const text = isRecord(body) && typeof body["text"] === "string" ? body["text"].trim() : "";

  if (!text) {
    return json<ErrorBody>({ error: "Enter text to summarize." }, 400);
  }

  const result = await summarizeText(text, readCloudflareEnv());
  return json<SummaryResult>(result);
}

async function readJson(request: Request): Promise<unknown> {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

function readCloudflareEnv(): AiEnv {
  try {
    return getCloudflareContext<AiEnv>().env;
  } catch {
    return {};
  }
}

function json<T>(body: T, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "cache-control": "no-store",
      "content-type": "application/json; charset=utf-8",
    },
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
