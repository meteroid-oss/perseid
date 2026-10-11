// `search`: the operations of the SDK whose words match a query, with the declarations of the
// types they take and return, read from the TypeScript sources the SDK's package ships.

export interface Operation {
  /** `things.attachments.upload`, called as `client.things.attachments.upload(...)`. */
  accessor: string;
  signature: string;
  method: string;
  path: string;
  summary: string;
  description: string;
  deprecated: boolean;
  /** A list: awaited, its first page; with `for await`, every item. */
  paginated: boolean;
  /** The types it takes and returns, and the SDK source declaring each. */
  types: [string, string][];
}

export interface Corpus {
  /** The npm package of the SDK, whose `src/` holds the declarations. */
  package: string;
  operations: Operation[];
  /** Every model, and the SDK source declaring it. */
  models: Record<string, string>;
}

/** The text of an SDK source by its path in the package (`src/resources/things.ts`), if found.
 * On Node, `perseid-mcp/node` reads the installed package; a Worker bundles them. */
export type Sources = (file: string) => string | undefined;

/** The operations and models matching `query`, best first, as text for a model to read. */
export function search(corpus: Corpus, query: string, limit = 5, read: Sources = () => undefined): string {
  const words = tokens(query);
  const scored = corpus.operations
    .map((operation) => ({ operation, score: score(words, query, operation) }))
    .filter(({ score }) => score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, limit);
  const models = Object.keys(corpus.models)
    .filter((name) => words.some((word) => word === name.toLowerCase() || tokens(name).includes(word)))
    .filter((name) => !scored.some(({ operation }) => operation.types.some(([type]) => type === name)))
    .slice(0, 3);
  if (!scored.length && !models.length) {
    const resources = [...new Set(corpus.operations.map((o) => o.accessor.split(".").slice(0, -1).join(".")))];
    return `Nothing matches "${query}". The resources are: ${resources.map((r) => `client.${r}`).join(", ")}.`;
  }
  const sources = new Declarations(read);
  const parts = scored.map(({ operation }) => {
    const notes = [operation.deprecated ? "Deprecated." : "", operation.paginated ? PAGES : ""].filter(Boolean);
    const about = [operation.description || operation.summary, ...notes].filter(Boolean).join(" ");
    const types = operation.types
      .map(([name, file]) => sources.declaration(name, file))
      .filter(Boolean)
      .join("\n\n");
    return [
      `## client.${operation.signature}`,
      `\`${operation.method} ${operation.path}\`${about ? ` ${about}` : ""}`,
      types ? "```ts\n" + types + "\n```" : "",
    ]
      .filter(Boolean)
      .join("\n\n");
  });
  for (const name of models) {
    const declaration = sources.declaration(name, corpus.models[name] ?? "");
    if (declaration) parts.push(`## ${name}\n\n\`\`\`ts\n${declaration}\n\`\`\``);
  }
  return parts.join("\n\n");
}

const PAGES = "A list: `await` gives its first page (`items`, `hasNextPage`), `for await` every item.";

function score(words: string[], query: string, operation: Operation): number {
  if (query.trim().replace(/^client\./, "").replace(/\(.*$/, "") === operation.accessor) return 100;
  const fields: [string, number][] = [
    [operation.accessor, 4],
    [operation.path, 2],
    [operation.summary, 2],
    [operation.description, 1],
    [operation.types.map(([name]) => name).join(" "), 1],
  ];
  let total = 0;
  for (const word of words) {
    for (const [text, weight] of fields) {
      if (tokens(text).includes(word)) total += weight;
    }
  }
  return total - (operation.deprecated ? 0.5 : 0);
}

/** Lowercased words, camelCase, snake_case and paths split, plural `s` dropped. */
export function tokens(text: string): string[] {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((word) => word.length > 1)
    .map((word) => (word.length > 3 && word.endsWith("s") ? word.slice(0, -1) : word));
}

/** The SDK's TypeScript sources, read once each. */
class Declarations {
  private files = new Map<string, string>();

  constructor(private read: Sources) {}

  /** The `export interface` or `export type` of `name` in `file`, its doc comment included. */
  declaration(name: string, file: string): string {
    if (!file) return "";
    let text = this.files.get(file);
    if (text === undefined) {
      try {
        text = this.read(file) ?? "";
      } catch {
        text = "";
      }
      this.files.set(file, text);
    }
    const found = new RegExp(`^export (interface|type) ${name}\\b`, "m").exec(text);
    if (!found) return "";
    const start = found.index;
    const doc = text.slice(0, start).match(/\/\*\*(?:(?!\*\/)[\s\S])*\*\/\s*$/);
    const from = doc ? start - doc[0].length : start;
    const rest = text.slice(start);
    const end = rest.search(found[1] === "interface" ? /^}/m : /;\s*$/m);
    return text.slice(from, start + (end < 0 ? rest.length : end + 1)).trim();
  }
}
