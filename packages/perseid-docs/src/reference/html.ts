import { Marked } from "marked";

const ESCAPES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

export function esc(text: unknown): string {
  return String(text ?? "").replace(/[&<>"']/g, (c) => ESCAPES[c]!);
}

/** Highlights `code` as `grammar`, returning the HTML inside `<code>`. */
export type Highlighter = (code: string, grammar: string) => string;

export const plain: Highlighter = (code) => esc(code);

const marked = new Marked({ gfm: true, breaks: false });

/** Markdown from the spec, which the API's owner wrote: rendered as is. */
export function md(text: string | undefined): string {
  return text ? (marked.parse(text, { async: false }) as string) : "";
}

export function mdInline(text: string | undefined): string {
  return text ? (marked.parseInline(text, { async: false }) as string) : "";
}

export function attrs(values: Record<string, string | number | boolean | undefined>): string {
  return Object.entries(values)
    .filter(([, v]) => v !== undefined && v !== false)
    .map(([k, v]) => (v === true ? ` ${k}` : ` ${k}="${esc(v)}"`))
    .join("");
}
