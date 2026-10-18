function httpFailure(response) {
  const status = response.status;
  if (status === 429) return "rate-limited";
  if (status === 403) {
    const remaining = response.headers?.get?.("x-ratelimit-remaining");
    const retryAfter = response.headers?.get?.("retry-after");
    if (remaining === "0" || /^\d+$/.test(retryAfter ?? "")) return "rate-limited";
    return "forbidden";
  }
  if (status === 401) return "unauthorized";
  if (status === 404) return "not-found";
  if (status >= 500 && status <= 599) return "server-error";
  return "http-error";
}

export async function referenceRequest(url, fetcher = fetch, env = process.env) {
  let target;
  try {
    target = new URL(url);
  } catch {
    throw Error("Reference acquisition: invalid endpoint");
  }
  if (
    target.protocol !== "https:" ||
    target.username ||
    target.password ||
    target.port ||
    !["api.github.com", "registry.npmjs.org", "codeload.github.com"].includes(target.hostname)
  )
    throw Error("Reference acquisition: invalid endpoint");
  const headers = {
    accept: "application/vnd.github+json, application/json, application/octet-stream",
  };
  if (target.origin === "https://api.github.com") {
    const token = env.GH_TOKEN || env.GITHUB_TOKEN;
    if (token) headers.authorization = `Bearer ${token}`;
  }
  let response;
  try {
    response = await fetcher(url, { headers, redirect: "error" });
  } catch {
    // Transport errors may include request headers; never retain their message/cause.
    throw Error("Reference acquisition: network failure (offline or transport error)");
  }
  if (response.redirected || (response.url && response.url !== url))
    throw Error("Reference acquisition: unexpected redirect");
  if (!response.ok || response.status !== 200) {
    const status = Number.isInteger(response.status) ? response.status : "invalid";
    throw Error(`Reference acquisition: HTTP ${status} (${httpFailure(response)})`);
  }
  return response;
}

export async function referenceBody(response, method) {
  try {
    return await response[method]();
  } catch {
    throw Error(
      `Reference acquisition: ${method === "json" ? "invalid JSON or body read failure" : "body read failure"}`,
    );
  }
}
