// The reference from an OpenAPI URL alone, as Scalar embeds it: one script, no build. With the
// docs data `perseid docs-data` writes, it follows the reader's SDK language too.
//
//   <script src="https://cdn.jsdelivr.net/npm/perseid-docs/dist/embed.js"
//           data-spec="https://api.acme.com/openapi.json" data-data="docs-data.json"></script>
import "./manual.js";
import "../src/reference/client.js";
import css from "../styles/reference.css";
import type { DocsData } from "../src/build/data.js";
import { assemble } from "../src/build/reference.js";
import { parseSpec } from "../src/build/spec.js";
import { languageCss, PRE_PAINT, renderLayout } from "../src/reference/index.js";
import { lightTheme, type ShikiTheme } from "../src/reference/shiki-theme.js";

const SHIKI = "https://esm.sh/shiki@4.5.0";

interface Options {
  spec: string;
  data?: string | undefined;
  target?: Element | undefined;
  baseUrl?: string | undefined;
}

async function text(url: string): Promise<string> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.text();
}

export async function render(options: Options): Promise<void> {
  new Function(PRE_PAINT)();
  const target = options.target ?? document.body.appendChild(document.createElement("div"));
  target.innerHTML = `<div class="pr-root pr-loading">Loading the API reference…</div>`;
  try {
    const [spec, data] = await Promise.all([
      text(options.spec).then((t) => parseSpec(t, options.spec)),
      options.data ? text(options.data).then((t) => JSON.parse(t) as DocsData) : undefined,
    ]);
    const reference = assemble(spec, data, options.baseUrl ? { baseUrl: options.baseUrl } : {});
    const style = document.createElement("style");
    style.textContent = `${css}\n${languageCss(reference)}`;
    document.head.append(style);
    target.innerHTML = renderLayout(reference);
    (globalThis as { perseidReference?: () => void }).perseidReference?.();
    if (location.hash) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();
    void highlight(target);
  } catch (e) {
    target.innerHTML = `<div class="pr-root pr-loading pr-try-error">Could not load the API reference: ${String(e instanceof Error ? e.message : e).replace(/</g, "&lt;")}</div>`;
  }
}

/** Highlights the code blocks after the first paint, with Shiki from a CDN. */
async function highlight(root: Element): Promise<void> {
  type Shiki = {
    codeToHtml: (code: string, o: object) => Promise<string>;
    bundledThemes: Record<string, () => Promise<{ default: ShikiTheme }>>;
  };
  let shiki: Shiki;
  let light: ShikiTheme;
  try {
    shiki = (await import(/* @vite-ignore */ SHIKI)) as Shiki;
    light = lightTheme((await shiki.bundledThemes["github-light"]!()).default);
  } catch {
    return;
  }
  const lang = (g: string) => ({ ts: "typescript", bash: "shellscript" })[g] ?? g;
  const paint = async (code: HTMLElement) => {
    const grammar = code.className.replace("language-", "");
    try {
      const html = await shiki.codeToHtml(code.textContent ?? "", {
        lang: lang(grammar),
        themes: { light, dark: "vesper" },
        defaultColor: false,
      });
      code.innerHTML = html.replace(/^<pre[^>]*><code>/, "").replace(/<\/code><\/pre>$/, "");
    } catch {}
  };
  // Only what comes near the screen: a big API has thousands of blocks.
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        void paint(entry.target as HTMLElement);
      }
    },
    { rootMargin: "800px 0px" },
  );
  for (const code of Array.from(root.querySelectorAll<HTMLElement>("code[class^='language-']"))) observer.observe(code);
}

const script = document.currentScript as HTMLScriptElement | null;
const params = new URLSearchParams(location.search);
const spec = script?.dataset.spec ?? params.get("spec");
if (spec) {
  const run = () =>
    render({
      spec,
      data: script?.dataset.data ?? params.get("data") ?? undefined,
      target: script?.dataset.target ? (document.querySelector(script.dataset.target) ?? undefined) : undefined,
      baseUrl: script?.dataset.baseUrl ?? params.get("base_url") ?? undefined,
    });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run);
  else void run();
}
(globalThis as { PerseidReference?: object }).PerseidReference = { render };
