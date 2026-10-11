import { createHighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import type { Highlighter } from "./reference/index.js";
import { lightTheme } from "./reference/shiki-theme.js";

const ALIASES: Record<string, string> = { ts: "typescript", bash: "shellscript", sh: "shellscript" };

/** Shiki with a light and a dark theme on every token, which the stylesheet switches. Themes
 * and grammars are imported by name, so a bundler keeps them. */
export async function shikiHighlighter(): Promise<Highlighter> {
  const shiki = await createHighlighterCore({
    themes: [import("shiki/dist/themes/github-light.mjs").then((m) => lightTheme(m.default)), import("shiki/dist/themes/vesper.mjs")],
    langs: [
      import("shiki/dist/langs/typescript.mjs"),
      import("shiki/dist/langs/python.mjs"),
      import("shiki/dist/langs/go.mjs"),
      import("shiki/dist/langs/java.mjs"),
      import("shiki/dist/langs/csharp.mjs"),
      import("shiki/dist/langs/rust.mjs"),
      import("shiki/dist/langs/shellscript.mjs"),
      import("shiki/dist/langs/json.mjs"),
    ],
    engine: createJavaScriptRegexEngine(),
  });
  const loaded = new Set(shiki.getLoadedLanguages());
  const themes = { light: "perseid-light", dark: "vesper" };
  return (code, grammar) => {
    const name = ALIASES[grammar] ?? grammar;
    const lang = loaded.has(name) ? name : "text";
    const html = shiki.codeToHtml(code, { lang, themes, defaultColor: false });
    return html.replace(/^<pre[^>]*><code>/, "").replace(/<\/code><\/pre>$/, "");
  };
}
