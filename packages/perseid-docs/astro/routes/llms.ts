import type { APIRoute } from "astro";
import { getCollection } from "astro:content";
import config from "virtual:perseid-docs/config";
import { loadRoutes, markdownPages } from "perseid-docs/routes";

/** llms.txt: every page with its Markdown URL and description, the convention agents look for. */
export const GET: APIRoute = async ({ site }) => {
  const origin = site ? site.origin : "";
  const docs = (await getCollection("docs")).sort((a, b) => a.id.localeCompare(b.id));
  const lines = [`# ${config.title}`, "", ...(config.description ? [`> ${config.description}`, ""] : []), "## Docs", ""];
  for (const e of docs) {
    const md = `${origin}/${e.id === "index" ? "index" : e.id}.md`;
    lines.push(`- [${e.data.title}](${md})${e.data.description ? `: ${e.data.description}` : ""}`);
  }
  if (config.reference) {
    lines.push("", "## API reference", "");
    for (const page of markdownPages(loadRoutes(config.reference, config.base))) {
      lines.push(`- [${page.title}](${origin}/${page.path}.md)${page.description ? `: ${page.description}` : ""}`);
    }
  }
  return new Response(`${lines.join("\n")}\n`, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
