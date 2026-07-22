import type { ReportPayload } from "./identity.ts";
import { sanitizeReport } from "./identity.ts";

const DEFAULT_TIMEOUT_MS = 5_000;

export async function postReport(
  endpoint: string,
  payload: ReportPayload,
  opts?: { token?: string; timeoutMs?: number },
): Promise<{ ok: boolean; status: number; body: string }> {
  const clean = sanitizeReport(payload);
  if (!clean) {
    return { ok: false, status: 0, body: "invalid payload" };
  }

  const body: ReportPayload = {
    ext_name: clean.ext_name,
    package_dir: clean.package_dir,
    kind: clean.kind,
  };
  if (payload.client) body.client = String(payload.client).slice(0, 32);
  if (payload.grok_pi_ver)
    body.grok_pi_ver = String(payload.grok_pi_ver).slice(0, 32);

  const ctrl = new AbortController();
  const t = setTimeout(
    () => ctrl.abort(),
    opts?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
  );
  try {
    const res = await fetch(endpoint.replace(/\/$/, "") + "/v1/report", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(opts?.token ? { authorization: `Bearer ${opts.token}` } : {}),
      },
      body: JSON.stringify(body),
      signal: ctrl.signal,
    });
    const text = await res.text();
    return { ok: res.ok, status: res.status, body: text };
  } catch (e) {
    return {
      ok: false,
      status: 0,
      body: e instanceof Error ? e.message : String(e),
    };
  } finally {
    clearTimeout(t);
  }
}

export async function fetchStats(
  endpoint: string,
  opts?: { token?: string },
): Promise<unknown> {
  const res = await fetch(endpoint.replace(/\/$/, "") + "/v1/stats", {
    headers: opts?.token
      ? { authorization: `Bearer ${opts.token}` }
      : undefined,
  });
  if (!res.ok) throw new Error(`stats ${res.status}: ${await res.text()}`);
  return res.json();
}
