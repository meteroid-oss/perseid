/** Prefixes root-relative links and sources of Markdown and MDX pages with the site's `base`,
 * which Astro leaves as written: `/billing/plans` becomes `/docs/billing/plans`. */

type Node = {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  attributes?: { type: string; name?: string; value?: unknown }[];
  children?: Node[];
};

const ATTRIBUTES = new Set(["href", "src", "poster", "srcset"]);

export function withBase(url: string, prefix: string): string {
  if (!prefix || !url.startsWith("/") || url.startsWith("//")) return url;
  if (url === prefix || url.startsWith(`${prefix}/`) || url.startsWith(`${prefix}?`) || url.startsWith(`${prefix}#`)) return url;
  return `${prefix}${url}`;
}

function rewrite(value: unknown, name: string, prefix: string): unknown {
  if (typeof value !== "string") return value;
  if (name === "srcset" || name === "srcSet") {
    return value
      .split(",")
      .map((part) => part.trim().replace(/^\S+/, (url) => withBase(url, prefix)))
      .join(", ");
  }
  return withBase(value, prefix);
}

function visit(node: Node, prefix: string) {
  if (node.properties) {
    for (const [name, value] of Object.entries(node.properties)) {
      if (ATTRIBUTES.has(name.toLowerCase())) node.properties[name] = rewrite(value, name, prefix);
    }
  }
  for (const attribute of node.attributes ?? []) {
    if (attribute.type === "mdxJsxAttribute" && attribute.name && ATTRIBUTES.has(attribute.name.toLowerCase())) {
      attribute.value = rewrite(attribute.value, attribute.name, prefix);
    }
  }
  for (const child of node.children ?? []) visit(child, prefix);
}

export default function rehypeBaseLinks(options: { base: string }) {
  const prefix = options.base.replace(/\/+$/, "");
  return (tree: Node) => {
    if (prefix) visit(tree, prefix);
  };
}

type Ctx = { setProperty(node: unknown, key: string, value: unknown): void; setAttribute(node: unknown, name: string, value: unknown): void };

/** The same, as a hast plugin of Sätteri, Astro's default Markdown processor. */
export function satteriBaseLinks(options: { base: string }) {
  const prefix = options.base.replace(/\/+$/, "");
  const jsx = {
    filter: [],
    visit(node: Node, ctx: Ctx) {
      for (const attribute of node.attributes ?? []) {
        if (attribute.type !== "mdxJsxAttribute" || !attribute.name || !ATTRIBUTES.has(attribute.name.toLowerCase())) continue;
        const value = rewrite(attribute.value, attribute.name, prefix);
        if (value !== attribute.value) ctx.setAttribute(node, attribute.name, value);
      }
    },
  };
  return {
    name: "perseid-docs-base-links",
    element: {
      filter: [],
      visit(node: Node, ctx: Ctx) {
        for (const [name, value] of Object.entries(node.properties ?? {})) {
          if (!ATTRIBUTES.has(name.toLowerCase())) continue;
          const next = rewrite(value, name, prefix);
          if (next !== value) ctx.setProperty(node, name, next);
        }
      },
    },
    mdxJsxFlowElement: jsx,
    mdxJsxTextElement: jsx,
  };
}

type Processor = { name?: string; options?: { hastPlugins?: unknown[]; rehypePlugins?: unknown[] } };

/** Registers the plugin with the site's Markdown processor, which MDX shares. */
export function registerBaseLinks(processor: Processor | undefined, base: string): boolean {
  if (!base.replace(/\/+$/, "") || !processor?.options) return true;
  if (Array.isArray(processor.options.hastPlugins)) processor.options.hastPlugins.push(satteriBaseLinks({ base }));
  else if (Array.isArray(processor.options.rehypePlugins)) processor.options.rehypePlugins.push([rehypeBaseLinks, { base }]);
  else return false;
  return true;
}
