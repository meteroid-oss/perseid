import type { APIRoute, GetStaticPaths } from "astro";
import { getCollection } from "astro:content";
import config from "virtual:perseid-docs/config";
import { loadRoutes, markdownPages } from "perseid-docs/routes";

/** Every page as Markdown, at its URL plus `.md`: what "Copy page" copies and agents read. */
export const getStaticPaths: GetStaticPaths = async () => {
  const docs = await getCollection("docs");
  const pages = docs.map((entry) => ({
    params: { slug: entry.id === "index" ? "index" : entry.id },
    props: { text: guide(entry.data.title, entry.data.description, entry.body ?? "") },
  }));
  if (config.reference) {
    for (const page of markdownPages(loadRoutes(config.reference, config.base))) {
      pages.push({ params: { slug: page.path }, props: { text: page.text } });
    }
  }
  return pages;
};

export const GET: APIRoute = ({ props }) =>
  new Response(props.text as string, { headers: { "Content-Type": "text/markdown; charset=utf-8" } });

function guide(title: string, description: string | undefined, body: string): string {
  const text = body.replace(/^import\s.+?;?\s*$/gm, "").replace(/<div class="pd-frame-page" \/>\n*/g, "").trim();
  return `# ${title}\n\n${description ? `> ${description}\n\n` : ""}${text}\n`;
}
