// MyTune licence-key server (Cloudflare Worker + D1). Secrets: ADMIN_SECRET, SIGNING_KEY (Ed25519 PKCS8 DER, base64).

const DAY = 86400;
const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // 32 symbols: a byte & 31 has no bias

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const fail = (error, status) => json({ error }, status);
const now = () => Math.floor(Date.now() / 1000);

function b64url(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64decode(text) {
  const s = atob(text.trim());
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

export async function makeToken(signingKeyB64, key, device, exp) {
  const pk = await crypto.subtle.importKey("pkcs8", b64decode(signingKeyB64), { name: "Ed25519" }, false, ["sign"]);
  const payload = new TextEncoder().encode(JSON.stringify({ key, device, exp }));
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pk, payload));
  return `${b64url(payload)}.${b64url(sig)}`;
}

export function newKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(20));
  const chars = Array.from(bytes, (b) => ALPHABET[b & 31]).join("");
  return chars.match(/.{5}/g).join("-");
}

// Constant time: compares SHA-256 digests so length differences leak nothing either.
async function sameSecret(a, b) {
  const enc = new TextEncoder();
  const [x, y] = await Promise.all([
    crypto.subtle.digest("SHA-256", enc.encode(a)),
    crypto.subtle.digest("SHA-256", enc.encode(b)),
  ]);
  const u = new Uint8Array(x), v = new Uint8Array(y);
  let diff = 0;
  for (let i = 0; i < u.length; i++) diff |= u[i] ^ v[i];
  return diff === 0;
}

async function body(request) {
  try {
    const value = await request.json();
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

const validKey = (k) => typeof k === "string" && k.length > 0 && k.length <= 40;

async function activate(request, env) {
  const b = await body(request);
  if (!b || !validKey(b.key) || typeof b.device !== "string" || !/^[0-9a-fA-F]{64}$/.test(b.device)
      || (b.name !== undefined && typeof b.name !== "string")) {
    return fail("bad request", 400);
  }
  const key = b.key, device = b.device, name = (b.name || "").slice(0, 64);
  const row = await env.DB.prepare("SELECT revoked, device FROM keys WHERE key = ?").bind(key).first();
  if (!row) return fail("invalid", 403);
  if (row.revoked) return fail("revoked", 403);
  const t = now();
  if (row.device === null) {
    // One conditional UPDATE: of two PCs racing, only one gets a changed row.
    const r = await env.DB.prepare(
      "UPDATE keys SET device = ?, device_name = ?, bound_at = ? WHERE key = ? AND device IS NULL AND revoked = 0"
    ).bind(device, name, t, key).run();
    if (!r.meta.changes) {
      const again = await env.DB.prepare("SELECT revoked, device FROM keys WHERE key = ?").bind(key).first();
      if (!again) return fail("invalid", 403);
      if (again.revoked) return fail("revoked", 403);
      if (again.device !== device) return fail("bound", 403);
    }
  } else if (row.device !== device) {
    return fail("bound", 403);
  }
  await env.DB.prepare("UPDATE keys SET last_seen = ? WHERE key = ?").bind(t, key).run();
  return json({ token: await makeToken(env.SIGNING_KEY, key, device, t + 7 * DAY) });
}

async function admin(path, request, env) {
  if (request.method === "GET" && path === "/admin/keys") {
    const { results } = await env.DB.prepare("SELECT * FROM keys ORDER BY created DESC, key").all();
    return json(results.map((r) => ({ ...r, revoked: !!r.revoked })));
  }
  if (request.method !== "POST") return fail("not found", 404);
  const b = await body(request);
  if (!b) return fail("bad request", 400);

  if (path === "/admin/keys") {
    if (b.note !== undefined && typeof b.note !== "string") return fail("bad request", 400);
    const key = newKey();
    await env.DB.prepare("INSERT INTO keys (key, note, created) VALUES (?, ?, ?)")
      .bind(key, (b.note || "").slice(0, 200), now()).run();
    return json({ key });
  }

  const routes = {
    "/admin/revoke": () => typeof b.revoked === "boolean"
      && env.DB.prepare("UPDATE keys SET revoked = ? WHERE key = ?").bind(b.revoked ? 1 : 0, b.key),
    "/admin/unbind": () =>
      env.DB.prepare("UPDATE keys SET device = NULL, device_name = NULL, bound_at = NULL WHERE key = ?").bind(b.key),
    "/admin/delete": () => env.DB.prepare("DELETE FROM keys WHERE key = ?").bind(b.key),
  };
  if (!routes[path]) return fail("not found", 404);
  const stmt = validKey(b.key) && routes[path]();
  if (!stmt) return fail("bad request", 400);
  const r = await stmt.run();
  if (!r.meta.changes) return fail("no such key", 404);
  return json({ ok: true });
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    try {
      if (path === "/activate") {
        return request.method === "POST" ? await activate(request, env) : fail("not found", 404);
      }
      if (path.startsWith("/admin/")) {
        const auth = request.headers.get("authorization") || "";
        const ok = env.ADMIN_SECRET && auth.startsWith("Bearer ")
          && (await sameSecret(auth.slice(7), env.ADMIN_SECRET));
        if (!ok) return fail("unauthorized", 401);
        return await admin(path, request, env);
      }
      return fail("not found", 404);
    } catch (e) {
      return fail("server error", 500);
    }
  },
};
