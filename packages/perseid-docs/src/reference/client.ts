// The reference's behavior in the browser: language, copy, tabs, theme, search, try-it.
// Plain DOM, no dependency: the pages render fully without it. The embed renders first, then
// calls `perseidReference()`; a page with the markup already in it runs it at once.
function perseidReference() {
  const root = document.documentElement;
  const store = {
    get(key: string): string | null {
      try {
        return localStorage.getItem(key);
      } catch {
        return null;
      }
    },
    set(key: string, value: string) {
      try {
        localStorage.setItem(key, value);
      } catch {}
    },
  };
  const $$ = <T extends Element = HTMLElement>(sel: string, el: ParentNode = document) =>
    Array.from(el.querySelectorAll<T & Element>(sel)) as unknown as T[];

  const visible = (el: Element) => (el as HTMLElement).offsetParent !== null;

  function setLanguage(lang: string, persist = true) {
    root.dataset.prLang = lang;
    for (const b of $$<HTMLButtonElement>(".pr-lang")) b.setAttribute("aria-checked", String(b.dataset.lang === lang));
    const label = $$<HTMLButtonElement>(`.pr-lang[data-lang="${CSS.escape(lang)}"]`)[0]?.textContent ?? lang;
    for (const el of $$(".pr-card-lang")) el.textContent = label;
    for (const select of $$<HTMLSelectElement>(".pr-lang-select")) select.value = lang;
    if (persist) store.set("pr-lang", lang);
  }

  function initLanguage() {
    const known = [
      ...$$<HTMLButtonElement>(".pr-lang").map((b) => b.dataset.lang),
      ...Array.from($$<HTMLSelectElement>(".pr-lang-select")[0]?.options ?? []).map((o) => o.value),
    ];
    const saved = store.get("pr-lang");
    const lang = saved && known.includes(saved) ? saved : known[0];
    if (lang) setLanguage(lang, false);
  }

  async function copy(button: HTMLElement) {
    const card = button.closest(".pr-card");
    const pre = card && $$<HTMLPreElement>("pre", card).find((p) => visible(p) && !p.hidden);
    if (!pre) return;
    const setup = pre.dataset.setup;
    const text = setup ? `${setup}\n\n${pre.innerText}` : pre.innerText;
    try {
      await navigator.clipboard.writeText(text);
      button.dataset.copied = "";
      button.title = setup ? "Copied, with the client setup" : "Copied";
      setTimeout(() => delete button.dataset.copied, 1500);
    } catch {}
  }

  function selectTab(tab: HTMLElement) {
    const card = tab.closest(".pr-card");
    if (!card) return;
    for (const t of $$(".pr-tab", card)) t.setAttribute("aria-selected", String(t === tab));
    for (const p of $$<HTMLElement>("[data-panel]", card)) p.hidden = p.dataset.panel !== tab.dataset.tab;
  }

  function toggleTheme() {
    const dark = root.dataset.theme
      ? root.dataset.theme === "dark"
      : matchMedia("(prefers-color-scheme: dark)").matches;
    root.dataset.theme = dark ? "light" : "dark";
    store.set("pr-theme", root.dataset.theme);
  }

  // ⌘K search over the operations of the page.
  interface Entry {
    slug: string;
    title: string;
    group: string;
    method: string;
    path: string;
    sdk: string[];
  }
  let dialog: HTMLDialogElement | undefined;
  function openSearch() {
    const data = document.getElementById("pr-search-index");
    if (!data) return;
    const entries = JSON.parse(data.textContent ?? "[]") as Entry[];
    if (!dialog) {
      dialog = document.createElement("dialog");
      dialog.className = "pr-search pr-root";
      dialog.innerHTML = `<input class="pr-search-input" type="search" placeholder="Search operations, paths, methods…" aria-label="Search"><ul class="pr-search-results" role="listbox"></ul><div class="pr-search-foot"><span><kbd>↑</kbd><kbd>↓</kbd> to navigate</span><span><kbd>↵</kbd> to open</span><span><kbd>esc</kbd> to close</span></div>`;
      document.body.append(dialog);
      const input = dialog.querySelector("input")!;
      const list = dialog.querySelector("ul")!;
      let selected = 0;
      const render = () => {
        const terms = input.value.toLowerCase().split(/\s+/).filter(Boolean);
        const hits = entries
          .map((e) => {
            const hay = `${e.title} ${e.group} ${e.method} ${e.path} ${e.sdk.join(" ")}`.toLowerCase();
            const score = terms.every((t) => hay.includes(t)) ? terms.reduce((s, t) => s + (e.title.toLowerCase().includes(t) ? 2 : 1), 0) : -1;
            return { e, score };
          })
          .filter((h) => h.score >= 0)
          .sort((a, b) => b.score - a.score)
          .slice(0, 50);
        selected = Math.min(selected, Math.max(hits.length - 1, 0));
        list.innerHTML = hits
          .map(
            ({ e }, i) =>
              `<li role="option" aria-selected="${i === selected}"><a href="#${e.slug}"><span class="pr-method pr-m-${e.method}">${e.method.toUpperCase()}</span><span class="pr-search-title">${escapeHtml(e.title)}</span><span class="pr-search-path">${escapeHtml(e.group)} · ${escapeHtml(e.path)}</span></a></li>`,
          )
          .join("") || `<li class="pr-search-empty">No operation matches</li>`;
      };
      input.addEventListener("input", () => {
        selected = 0;
        render();
      });
      input.addEventListener("keydown", (ev) => {
        const items = $$("li[role=option]", list);
        if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
          ev.preventDefault();
          selected = (selected + (ev.key === "ArrowDown" ? 1 : -1) + items.length) % Math.max(items.length, 1);
          items.forEach((li, i) => li.setAttribute("aria-selected", String(i === selected)));
          items[selected]?.scrollIntoView({ block: "nearest" });
        } else if (ev.key === "Enter") {
          items[selected]?.querySelector("a")?.click();
        }
      });
      list.addEventListener("click", (ev) => {
        if ((ev.target as Element).closest("a")) dialog!.close();
      });
      dialog.addEventListener("click", (ev) => {
        if (ev.target === dialog) dialog!.close();
      });
      render();
    }
    dialog.showModal();
    const input = dialog.querySelector("input")!;
    input.select();
  }

  function escapeHtml(s: string) {
    return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
  }

  // The sidebar follows the operation in view.
  function scrollSpy() {
    const links = new Map($$<HTMLAnchorElement>(".pr-nav-op[data-slug]").map((a) => [a.dataset.slug!, a]));
    if (!links.size || !("IntersectionObserver" in window)) return;
    let current: HTMLAnchorElement | undefined;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const link = links.get(entry.target.id);
          if (!link || link === current) continue;
          current?.removeAttribute("aria-current");
          link.setAttribute("aria-current", "true");
          current = link;
          const group = link.closest("details");
          if (group && !group.open) group.open = true;
          const nav = link.closest(".pr-sidebar");
          if (nav) {
            const r = link.getBoundingClientRect();
            const n = nav.getBoundingClientRect();
            if (r.top < n.top + 40 || r.bottom > n.bottom - 40) link.scrollIntoView({ block: "center" });
          }
        }
      },
      { rootMargin: "-20% 0px -70% 0px" },
    );
    for (const section of $$(".pr-op")) observer.observe(section);
  }

  // Try it: a sheet sending the operation from the browser. The key stays in this tab unless the
  // reader asks to remember it on this device; the server, the values and the bodies typed in
  // follow the reader from one operation to the next.
  interface OpData {
    title: string;
    method: string;
    path: string;
    params: { name: string; in: string; required: boolean; type?: string; example?: string }[];
    contentType?: string;
    body?: unknown;
    auth: { kind: string; in?: string; name?: string }[];
  }
  const session = {
    get(key: string): string | null {
      try {
        return sessionStorage.getItem(key);
      } catch {
        return null;
      }
    },
    set(key: string, value: string) {
      try {
        sessionStorage.setItem(key, value);
      } catch {}
    },
  };
  // The key and the headers follow the "remember" switch: this tab only, or this device.
  const prefs = {
    remembered: () => store.get("pr-remember") === "1" || store.get("pr-key") !== null,
    get: (name: string) => (prefs.remembered() ? store.get(`pr-${name}`) : null) ?? session.get(`pr-${name}`) ?? "",
    set(name: string, value: string) {
      session.set(`pr-${name}`, value);
      if (prefs.remembered()) store.set(`pr-${name}`, value);
    },
    remember(on: boolean) {
      const values = { key: prefs.get("key"), headers: prefs.get("headers") };
      try {
        for (const name of ["remember", "key", "headers"]) localStorage.removeItem(`pr-${name}`);
      } catch {}
      if (on) store.set("pr-remember", "1");
      for (const [name, value] of Object.entries(values)) prefs.set(name, value);
    },
    headers(): [string, string][] {
      return prefs
        .get("headers")
        .split("\n")
        .map((line) => line.split(/:(.*)/s).map((part) => part.trim()))
        .filter((pair): pair is [string, string] => !!pair[0] && pair[1] !== undefined);
    },
  };
  function prefsNote(): string {
    return prefs.remembered()
      ? `Saved in this browser until you <button type="button" class="pr-client-forget">forget it</button>. Use a test-mode key.`
      : "Kept in this tab only. Requests go straight from your browser to the server.";
  }
  function reveal(button: HTMLButtonElement) {
    const input = button.closest(".pr-client-secret")?.querySelector("input");
    if (!input) return;
    const shown = input.type === "password";
    input.type = shown ? "text" : "password";
    button.setAttribute("aria-pressed", String(shown));
  }
  function settings() {
    for (const form of $$<HTMLFormElement>(".pr-settings")) {
      const field = (name: string) => form.querySelector<HTMLInputElement & HTMLTextAreaElement>(`[name="${name}"]`);
      const note = form.querySelector<HTMLElement>(".pr-client-note")!;
      const server = field("__server");
      if (server) server.value = store.get("pr-server") ?? server.value;
      const key = field("__key");
      if (key) key.value = prefs.get("key");
      field("__headers")!.value = prefs.get("headers");
      const remember = field("__remember");
      if (remember) remember.checked = prefs.remembered();
      note.innerHTML = prefsNote();
      form.addEventListener("submit", (ev) => ev.preventDefault());
      form.addEventListener("input", (ev) => {
        const input = ev.target as HTMLInputElement;
        if (input.name === "__server") store.set("pr-server", input.value.replace(/\/$/, ""));
        if (input.name === "__key") prefs.set("key", input.value);
        if (input.name === "__headers") prefs.set("headers", input.value);
      });
      form.addEventListener("change", (ev) => {
        if ((ev.target as HTMLInputElement).name !== "__remember") return;
        prefs.remember(!!remember?.checked);
        note.innerHTML = prefsNote();
      });
      form.addEventListener("click", (ev) => {
        const target = ev.target as Element;
        const button = target.closest<HTMLButtonElement>(".pr-client-reveal");
        if (button) return reveal(button);
        if (target.closest(".pr-client-forget")) {
          prefs.remember(false);
          if (remember) remember.checked = false;
          note.innerHTML = prefsNote();
        }
      });
    }
  }
  const COPY_ICON = `<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path fill="currentColor" d="M5 2.5A1.5 1.5 0 0 1 6.5 1h6A1.5 1.5 0 0 1 14 2.5v6a1.5 1.5 0 0 1-1.5 1.5H11v1.5A1.5 1.5 0 0 1 9.5 13h-6A1.5 1.5 0 0 1 2 11.5v-6A1.5 1.5 0 0 1 3.5 4H5V2.5ZM6.5 2a.5.5 0 0 0-.5.5V4h3.5A1.5 1.5 0 0 1 11 5.5V9h1.5a.5.5 0 0 0 .5-.5v-6a.5.5 0 0 0-.5-.5h-6Zm-3 3a.5.5 0 0 0-.5.5v6a.5.5 0 0 0 .5.5h6a.5.5 0 0 0 .5-.5v-6a.5.5 0 0 0-.5-.5h-6Z"/></svg>`;
  const CLOSE_ICON = `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8"/></svg>`;
  const EYE_ICON = `<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8Z"/><circle cx="8" cy="8" r="2"/></svg>`;
  const IN_TITLES: Record<string, string> = { path: "Path", query: "Query", header: "Headers", cookie: "Cookies" };

  function highlightJson(text: string): string {
    const token = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b/g;
    let out = "";
    let last = 0;
    for (const m of text.matchAll(token)) {
      out += escapeHtml(text.slice(last, m.index));
      const cls = m[1] ? (m[2] ? "pr-j-key" : "pr-j-str") : m[3] ? "pr-j-lit" : "pr-j-num";
      out += `<span class="${cls}">${escapeHtml(m[1] ?? m[0])}</span>${m[2] ? escapeHtml(m[2]) : ""}`;
      last = m.index! + m[0].length;
    }
    return out + escapeHtml(text.slice(last));
  }

  function bytes(n: number): string {
    return n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`;
  }

  let client: { el: HTMLElement; trigger: HTMLElement | null } | undefined;

  function closeClient() {
    if (!client) return;
    const { el, trigger } = client;
    client = undefined;
    el.classList.remove("pr-open");
    root.classList.remove("pr-client-lock");
    setTimeout(() => el.remove(), 260);
    trigger?.focus();
  }

  function tryIt(button: HTMLElement) {
    const section = button.closest(".pr-op");
    if (!section) return;
    const op = JSON.parse(section.querySelector(".pr-op-data")?.textContent ?? "{}") as OpData;
    const servers = JSON.parse(document.getElementById("pr-servers")?.textContent ?? "[]") as string[];
    const bodyKey = `pr-body:${op.method} ${op.path}`;
    const example = op.body === undefined ? "" : JSON.stringify(op.body, null, 2);
    const row = (p: OpData["params"][number]) => {
      const value = session.get(`pr-param:${p.in}:${p.name}`) ?? p.example ?? "";
      return `<label class="pr-client-row"><span class="pr-client-label"><code>${escapeHtml(p.name)}</code>${p.required ? `<i class="pr-client-req" title="Required">required</i>` : ""}${p.type ? `<span class="pr-client-type">${escapeHtml(p.type)}</span>` : ""}</span><input name="${escapeHtml(`${p.in}:${p.name}`)}" value="${escapeHtml(value)}" placeholder="${escapeHtml(p.example ?? (p.required ? "" : "optional"))}" autocomplete="off" spellcheck="false"${p.required ? " required" : ""}></label>`;
    };
    const groups = ["path", "query", "header", "cookie"]
      .map((where) => {
        const params = op.params.filter((p) => p.in === where);
        return params.length ? `<section class="pr-client-group"><h5>${IN_TITLES[where]}</h5><div class="pr-client-rows">${params.map(row).join("")}</div></section>` : "";
      })
      .join("");
    const remembered = prefs.remembered();
    const connection = `<section class="pr-client-group"><h5>Connection</h5><div class="pr-client-rows">
<label class="pr-client-row"><span class="pr-client-label">Server</span><input name="__server" list="pr-client-servers" value="${escapeHtml(store.get("pr-server") ?? servers[0] ?? location.origin)}" autocomplete="off" spellcheck="false" required></label>
${op.auth.length ? `<label class="pr-client-row"><span class="pr-client-label">API key</span><span class="pr-client-secret"><input name="__key" type="password" value="${escapeHtml(prefs.get("key"))}" placeholder="Paste a test-mode key" autocomplete="off" spellcheck="false"><button type="button" class="pr-client-reveal" aria-label="Show the key" aria-pressed="false">${EYE_ICON}</button></span></label>
<label class="pr-client-row pr-client-toggle"><span class="pr-client-label">Remember on this device</span><input type="checkbox" role="switch" name="__remember"${remembered ? " checked" : ""}></label>` : ""}
</div>${op.auth.length ? `<p class="pr-client-note"></p>` : ""}<datalist id="pr-client-servers">${servers.map((s) => `<option value="${escapeHtml(s)}">`).join("")}</datalist></section>`;
    const body = op.body !== undefined
      ? `<section class="pr-client-group"><h5>Body <span class="pr-client-ct">${escapeHtml(op.contentType ?? "")}</span><button type="button" class="pr-client-reset">Reset</button></h5><textarea name="__body" class="pr-client-body" rows="10" spellcheck="false">${escapeHtml(session.get(bodyKey) ?? example)}</textarea></section>`
      : "";

    const el = document.createElement("div");
    el.className = "pr-client";
    el.innerHTML = `<div class="pr-client-backdrop"></div>
<div class="pr-client-sheet pr-root" role="dialog" aria-modal="true" aria-label="Try it: ${escapeHtml(op.title)}">
<header class="pr-client-head"><div class="pr-client-heading"><span class="pr-client-eyebrow">Try it</span><span class="pr-client-title">${escapeHtml(op.title)}</span></div><button type="button" class="pr-client-curl">Copy as cURL</button><button type="button" class="pr-client-close" aria-label="Close">${CLOSE_ICON}</button></header>
<form class="pr-client-form" novalidate>
<div class="pr-client-bar"><span class="pr-method pr-m-${escapeHtml(op.method)}">${escapeHtml(op.method === "delete" ? "DEL" : op.method.toUpperCase())}</span><code class="pr-client-url"></code><button class="pr-send" type="submit">Send<kbd>⌘↵</kbd></button></div>
<div class="pr-client-scroll">${connection}${groups}${body}<section class="pr-client-response" hidden aria-live="polite"></section></div>
</form></div>`;
    document.body.append(el);
    closeClient();
    client = { el, trigger: button };
    root.classList.add("pr-client-lock");
    requestAnimationFrame(() => requestAnimationFrame(() => el.classList.add("pr-open")));

    const form = el.querySelector<HTMLFormElement>("form")!;
    const value = (name: string) => form.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${CSS.escape(name)}"]`)?.value ?? "";
    const note = el.querySelector<HTMLElement>(".pr-client-note");
    const remember = form.querySelector<HTMLInputElement>('[name="__remember"]');

    function request() {
      const base = value("__server").replace(/\/$/, "");
      const path = op.path.replace(/\{([^}]+)\}/g, (_, n) => encodeURIComponent(value(`path:${n}`)));
      const query = new URLSearchParams();
      const headers: Record<string, string> = {};
      for (const p of op.params) {
        const v = value(`${p.in}:${p.name}`);
        if (!v) continue;
        if (p.in === "query") query.append(p.name, v);
        if (p.in === "header") headers[p.name] = v;
      }
      const key = value("__key");
      const scheme = op.auth[0];
      if (key && scheme) {
        if (scheme.kind === "basic") headers.Authorization = `Basic ${btoa(key.includes(":") ? key : `${key}:`)}`;
        else if (scheme.kind === "apiKey" && scheme.in === "query" && scheme.name) query.set(scheme.name, key);
        else if (scheme.kind === "apiKey" && scheme.name) headers[scheme.name] = key;
        else headers.Authorization = `Bearer ${key}`;
      }
      for (const [name, v] of prefs.headers()) headers[name] = v;
      const body = op.body !== undefined ? value("__body") : undefined;
      if (body !== undefined) headers["Content-Type"] = op.contentType ?? "application/json";
      const search = [...query].length ? `?${query}` : "";
      return { url: base + path + search, base, search, headers, body };
    }

    function renderUrl() {
      const { base, search } = request();
      const path = escapeHtml(op.path).replace(/\{([^}]+)\}/g, (whole, n) => {
        const v = value(`path:${n}`);
        return v ? `<span class="pr-client-filled">${escapeHtml(encodeURIComponent(v))}</span>` : `<span class="pr-client-missing">${whole}</span>`;
      });
      el.querySelector(".pr-client-url")!.innerHTML = `<span class="pr-client-origin">${escapeHtml(base)}</span>${path}<span class="pr-client-search">${escapeHtml(search)}</span>`;
    }

    function renderNote() {
      if (!note) return;
      const extra = prefs.headers().length;
      note.innerHTML = prefsNote() + (extra ? ` Plus ${extra} header${extra > 1 ? "s" : ""} from the introduction.` : "");
    }

    form.addEventListener("input", (ev) => {
      const input = ev.target as HTMLInputElement;
      if (input.name === "__body") session.set(bodyKey, input.value);
      else if (input.name === "__key") prefs.set("key", input.value);
      else if (input.name.includes(":")) session.set(`pr-param:${input.name}`, input.value);
      renderUrl();
    });
    form.addEventListener("change", (ev) => {
      if ((ev.target as HTMLInputElement).name !== "__remember") return;
      prefs.remember(!!remember?.checked);
      renderNote();
    });
    form.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" && (ev.metaKey || ev.ctrlKey)) {
        ev.preventDefault();
        form.requestSubmit();
      }
    });
    el.addEventListener("click", (ev) => {
      const target = ev.target as Element;
      if (target.closest(".pr-client-close") || target.classList.contains("pr-client-backdrop")) return closeClient();
      if (target.closest(".pr-client-forget")) {
        prefs.remember(false);
        if (remember) remember.checked = false;
        renderNote();
        return;
      }
      const revealButton = target.closest<HTMLButtonElement>(".pr-client-reveal");
      if (revealButton) return reveal(revealButton);
      if (target.closest(".pr-client-reset")) {
        form.querySelector<HTMLTextAreaElement>('[name="__body"]')!.value = example;
        session.set(bodyKey, example);
        return;
      }
      const curl = target.closest<HTMLButtonElement>(".pr-client-curl");
      if (curl) {
        const { url, headers, body } = request();
        const key = value("__key");
        const lines = [`curl -X ${op.method.toUpperCase()} '${url.replaceAll(key || "\0", "$API_KEY")}'`];
        for (const [name, v] of Object.entries(headers)) {
          const shown = key && v.includes(key) ? v.replace(key, "$API_KEY") : name === "Authorization" && v.startsWith("Basic") ? "Basic $API_KEY" : v;
          lines.push(`-H "${name}: ${shown}"`);
        }
        if (body) lines.push(`-d '${body.replace(/'/g, "'\\''")}'`);
        void navigator.clipboard?.writeText(lines.join(" \\\n  ")).then(() => {
          curl.textContent = "Copied";
          setTimeout(() => (curl.textContent = "Copy as cURL"), 1400);
        });
      }
    });

    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const missing = op.params.find((p) => p.required && !value(`${p.in}:${p.name}`));
      if (missing) {
        form.querySelector<HTMLInputElement>(`[name="${CSS.escape(`${missing.in}:${missing.name}`)}"]`)?.focus();
        return;
      }
      const { url, headers, body } = request();
      store.set("pr-server", value("__server").replace(/\/$/, ""));
      const result = el.querySelector<HTMLElement>(".pr-client-response")!;
      const send = form.querySelector<HTMLButtonElement>(".pr-send")!;
      result.hidden = false;
      result.innerHTML = `<div class="pr-client-status"><span class="pr-client-spinner"></span>Sending…</div>`;
      send.disabled = true;
      const started = performance.now();
      try {
        const res = await fetch(url, { method: op.method.toUpperCase(), headers, body });
        const text = await res.text();
        const ms = Math.round(performance.now() - started);
        let pretty = text;
        let json = false;
        try {
          pretty = JSON.stringify(JSON.parse(text), null, 2);
          json = true;
        } catch {}
        const responseHeaders = [...res.headers].map(([k, v]) => `<div class="pr-client-header"><code>${escapeHtml(k)}</code><span>${escapeHtml(v)}</span></div>`).join("");
        result.innerHTML = `<div class="pr-client-status"><span class="pr-client-code pr-status-${String(res.status)[0]}">${res.status} ${escapeHtml(res.statusText)}</span><span>${ms} ms</span><span>${bytes(new Blob([text]).size)}</span></div>
<div class="pr-card pr-client-result"><div class="pr-card-head"><div class="pr-tabs" role="tablist"><button type="button" class="pr-tab" role="tab" data-tab="body" aria-selected="true">Body</button><button type="button" class="pr-tab" role="tab" data-tab="headers" aria-selected="false">Headers</button></div><button class="pr-copy" type="button" aria-label="Copy">${COPY_ICON}</button></div>
<div class="pr-card-body"><pre class="pr-code" data-panel="body"><code>${text ? (json ? highlightJson(pretty) : escapeHtml(pretty)) : '<span class="pr-client-empty">Empty body</span>'}</code></pre><div class="pr-client-headers" data-panel="headers" hidden>${responseHeaders || '<p class="pr-client-empty">The server exposes no headers to the browser.</p>'}</div></div></div>`;
      } catch {
        result.innerHTML = `<div class="pr-client-status pr-client-failed">No response. The server may not allow requests from this site (CORS), or it is unreachable.</div>`;
      } finally {
        send.disabled = false;
      }
      result.scrollIntoView({ block: "nearest", behavior: "smooth" });
    });

    renderUrl();
    renderNote();
    const first = op.params.find((p) => p.required && !value(`${p.in}:${p.name}`));
    const focus = first
      ? form.querySelector<HTMLInputElement>(`[name="${CSS.escape(`${first.in}:${first.name}`)}"]`)
      : op.auth.length && !value("__key") ? form.querySelector<HTMLInputElement>('[name="__key"]') : form.querySelector<HTMLButtonElement>(".pr-send");
    setTimeout(() => focus?.focus({ preventScroll: true }), 60);
  }

  document.addEventListener("click", (ev) => {
    const target = ev.target as Element;
    const lang = target.closest<HTMLElement>(".pr-lang");
    if (lang?.dataset.lang) return setLanguage(lang.dataset.lang);
    const copyButton = target.closest<HTMLElement>(".pr-copy");
    if (copyButton) return void copy(copyButton);
    const tab = target.closest<HTMLElement>(".pr-tab");
    if (tab) return selectTab(tab);
    if (target.closest(".pr-theme")) return toggleTheme();
    if (target.closest(".pr-search-button")) return openSearch();
    const unclamp = target.closest<HTMLElement>(".pr-unclamp");
    if (unclamp) return unclamp.closest(".pr-clamp")?.classList.add("pr-unclamped");
    const tryButton = target.closest<HTMLElement>(".pr-try");
    if (tryButton) return tryIt(tryButton);
  });
  document.addEventListener("change", (ev) => {
    const select = (ev.target as Element).closest?.<HTMLSelectElement>(".pr-lang-select");
    if (select) setLanguage(select.value);
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && client) return closeClient();
    const typing = (ev.target as Element).closest?.("input, textarea, select, [contenteditable]");
    if (!document.getElementById("pr-search-index")) return;
    if ((ev.key === "k" && (ev.metaKey || ev.ctrlKey)) || (ev.key === "/" && !typing)) {
      ev.preventDefault();
      openSearch();
    }
  });
  initLanguage();
  scrollSpy();
  settings();
}

(globalThis as { perseidReference?: () => void }).perseidReference = perseidReference;
if (!(globalThis as { perseidReferenceManual?: boolean }).perseidReferenceManual) perseidReference();
