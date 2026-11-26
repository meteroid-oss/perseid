// The broker's pages: the consent page where a user pastes their API key, and its error page.
// Self-contained HTML, light and dark, no script, never framed.
import type { Api } from "../server.js";

interface Consent {
  client: { client_id: string; client_name?: string; redirect_uris: string[]; dynamic: boolean };
  /** Where this authorization's code goes: the host the page names. */
  redirectUri: string;
  /** The signed authorization request, carried back by the form. */
  pending: string;
  keysUrl?: string;
  error?: string;
}

export function consentPage(api: Api, consent: Consent, status = 200): Response {
  const { client } = consent;
  const host = hostOf(consent.redirectUri);
  const name = client.client_name || host || "An application";
  const claim = client.dynamic
    ? `<p class="note">This name is the application's own claim: check that you started this connection from <strong>${escape(host)}</strong>.</p>`
    : `<p class="note">Identified by <strong>${escape(hostOf(client.client_id))}</strong>.</p>`;
  const where = consent.keysUrl
    ? ` <a href="${escape(consent.keysUrl)}" target="_blank" rel="noopener noreferrer">Find your API key</a>.`
    : "";
  return page(
    `Connect ${api.name}`,
    `<h1>Connect <strong>${escape(name)}</strong> to ${escape(api.name)}</h1>
<p><strong>${escape(name)}</strong> will call the ${escape(api.name)} API with your API key, for as long as you keep it connected. It returns to <strong>${escape(host)}</strong>.</p>
${claim}
<form method="post">
  <input type="hidden" name="pending" value="${escape(consent.pending)}">
  <label for="api_key">Your ${escape(api.name)} API key</label>
  <input id="api_key" name="api_key" type="password" autocomplete="off" spellcheck="false" autofocus required${consent.error ? ' aria-invalid="true" aria-describedby="error"' : ""}>
  ${consent.error ? `<p id="error" class="error" role="alert">${escape(consent.error)}</p>` : ""}
  <p class="hint">Stored encrypted on this server, and never shown to ${escape(name)}.${where}</p>
  <div class="actions">
    <button type="submit" name="action" value="deny" class="secondary" formnovalidate>Cancel</button>
    <button type="submit" name="action" value="allow">Connect</button>
  </div>
</form>`,
    status,
  );
}

export function errorPage(api: Api, message: string): Response {
  return page(`${api.name}: connection failed`, `<h1>Connection failed</h1>\n<p>${escape(message)}</p>`, 400);
}

function page(title: string, body: string, status: number): Response {
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>${escape(title)}</title>
<style>
:root { color-scheme: light dark; --bg: #f7f7f8; --card: #fff; --text: #1a1a1e; --muted: #5d5d66; --line: #d9d9de; --accent: #2f5bd3; --danger: #b42318; }
@media (prefers-color-scheme: dark) { :root { --bg: #111114; --card: #1b1b20; --text: #ececf1; --muted: #a0a0ab; --line: #34343c; --accent: #7c9cf5; --danger: #f97066; } }
* { box-sizing: border-box; }
body { margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 16px; background: var(--bg); color: var(--text); font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
main { width: 100%; max-width: 440px; background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 28px; }
h1 { font-size: 20px; line-height: 1.3; margin: 0 0 12px; font-weight: 600; }
p { margin: 0 0 12px; }
.note, .hint { color: var(--muted); font-size: 13px; }
label { display: block; font-weight: 600; margin: 20px 0 6px; }
input[type=password] { width: 100%; padding: 10px 12px; font: inherit; color: inherit; background: transparent; border: 1px solid var(--line); border-radius: 8px; }
input:focus-visible, button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.error { color: var(--danger); font-size: 13px; margin-top: 6px; }
.hint { margin-top: 8px; }
a { color: var(--accent); }
.actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 20px; }
button { font: inherit; font-weight: 600; padding: 9px 16px; border-radius: 8px; border: 1px solid var(--accent); background: var(--accent); color: #fff; cursor: pointer; }
button.secondary { background: transparent; color: var(--text); border-color: var(--line); }
</style>
</head>
<body><main>
${body}
</main></body>
</html>`;
  return new Response(html, {
    status,
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
      "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'",
      "x-frame-options": "DENY",
      "referrer-policy": "no-referrer",
    },
  });
}

function hostOf(uri: string): string {
  try {
    return new URL(uri).host;
  } catch {
    return uri;
  }
}

function escape(text: string): string {
  return text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
