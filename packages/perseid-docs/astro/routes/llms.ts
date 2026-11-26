import type { APIRoute } from "astro";
import { getCollection } from "astro:content";
import config from "virtual:perseid-docs/config";
import { loadRoutes, markdownHref, markdownPages } from "perseid-docs/routes";

/** llms.txt: every page with its Markdown URL and description, the convention agents look for. */
export const GET: APIRoute = async ({ site }) => {
  const origin = site ? site.origin : "";
  const docs = (await getCollection("docs")).sort((a, b) => a.id.localeCompare(b.id));
  const lines = [`# ${config.title}`, "", ...(config.description ? [`> ${config.description}`, ""] : []), "## Docs", ""];
  for (const e of docs) {
    lines.push(`- [${e.data.title}](${origin}${markdownHref(e.id, config.format)})${e.data.description ? `: ${e.data.description}` : ""}`);
  }
  for (const r of config.references) {
    lines.push("", `## ${r.label}`, "");
    for (const page of markdownPages(loadRoutes(r.source, r.base, config.format))) {
      lines.push(`- [${page.title}](${origin}${markdownHref(page.path, config.format)})${page.description ? `: ${page.description}` : ""}`);
    }
  }
  return new Response(`${lines.join("\n")}\n`, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
