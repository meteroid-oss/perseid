const OPEN: Record<string, string> = { "(": ")", "{": "}", "[": "]" };
const CLOSE = new Set(Object.values(OPEN));

/** Import lines, kept whole and followed by a blank line. */
const IMPORT = /^(import |use |from \S+ import |using )/;

/** Languages accepting a comma after the last argument of a call broken over lines. */
const CALL_TRAILING_COMMA = new Set(["typescript", "python", "go", "rust"]);

/** Breaks every line longer than `width` at the commas of its outermost bracket pair, one
 * argument per line, recursively: the one-line calls perseid writes, laid out the way a
 * formatter would. A lone object argument is hugged: `create({` … `})`. */
export function wrap(code: string, language: string, width = 54): string {
  const unit = language === "go" ? "\t" : ["csharp", "java", "python", "rust"].includes(language) ? "    " : "  ";
  const lines = code.split("\n");
  const imports = lines.findIndex((line) => !IMPORT.test(line));
  let start = Math.max(imports, 0);
  while (imports > 0 && lines[start] === "") start++;
  const body = lines.slice(start).flatMap((line) => wrapLine(line, width, unit, CALL_TRAILING_COMMA.has(language)));
  const head = imports > 0 ? [...lines.slice(0, imports), ""] : [];
  return [...head, ...body].join("\n");
}

/** Loop and branch headers, left whole: their parentheses are not a call's. */
const CONTROL = /^\s*(for|if|while|switch|foreach|await foreach)\b/;

function wrapLine(line: string, width: number, unit: string, callComma: boolean): string[] {
  if (line.replace(/\t/g, "    ").length <= width || CONTROL.test(line)) return [line];
  const chain = chainLinks(line);
  if (chain.length > 2) {
    const indent = /^\s*/.exec(line)![0];
    return [chain[0]!, ...chain.slice(1).map((link) => `${indent}${unit}${link}`)].flatMap((l) =>
      wrapLine(l, width, unit, callComma),
    );
  }
  const span = outermost(line);
  if (!span) return [line];
  let [open, close] = span;
  const inner = line.slice(open + 1, close).trim();
  const hug = line[open] === "(" && /^[{[]/.test(inner) && outermost(inner)?.[0] === 0 && outermost(inner)?.[1] === inner.length - 1;
  if (hug) {
    open = line.indexOf(inner[0]!, open);
    close = line.lastIndexOf(inner[inner.length - 1]!, close);
  }
  const args = splitTop(line.slice(open + 1, close)).map((a) => a.trim()).filter(Boolean);
  if (!args.length || (!hug && args.length === 1 && !/[({[]/.test(args[0]!))) return [line];
  const indent = /^\s*/.exec(line)![0];
  const comma = line[open] !== "(" || callComma;
  const body = args.flatMap((a, i) =>
    wrapLine(`${indent}${unit}${a}${i < args.length - 1 || comma ? "," : ""}`, width, unit, callComma),
  );
  return [line.slice(0, open + 1).trimEnd(), ...body, indent + line.slice(close).trimStart()];
}

/** `a.b(x).c(y)` as `["a.b(x)", ".c(y)"]`: a builder chain, split at each top-level `).`. */
function chainLinks(line: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote: string | undefined;
  let start = 0;
  for (let i = 0; i < line.length; i++) {
    const c = line[i]!;
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = undefined;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") quote = c;
    else if (OPEN[c]) depth++;
    else if (CLOSE.has(c)) depth--;
    else if (c === "." && depth === 0 && line[i - 1] === ")" && /[A-Za-z_]/.test(line[i + 1] ?? "")) {
      out.push(line.slice(start, i).trimStart() === line.slice(start, i) ? line.slice(start, i) : line.slice(start, i));
      start = i;
    }
  }
  out.push(line.slice(start));
  return out.filter((l, i) => i === 0 || l.trim());
}

/** The first bracket pair spanning the most text, outside strings. */
function outermost(line: string): [number, number] | undefined {
  const stack: [string, number][] = [];
  let best: [number, number] | undefined;
  let quote: string | undefined;
  for (let i = 0; i < line.length; i++) {
    const c = line[i]!;
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = undefined;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") quote = c;
    else if (OPEN[c]) stack.push([c, i]);
    else if (CLOSE.has(c)) {
      const top = stack.pop();
      if (!top || OPEN[top[0]] !== c) return best;
      if (stack.length === 0 && i - top[1] > 2 && (!best || i - top[1] > best[1] - best[0])) {
        best = [top[1], i];
      }
    }
  }
  return best;
}

function splitTop(text: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote: string | undefined;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = undefined;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") quote = c;
    else if (OPEN[c]) depth++;
    else if (CLOSE.has(c)) depth--;
    else if (c === "," && depth === 0) {
      out.push(text.slice(start, i));
      start = i + 1;
    }
  }
  out.push(text.slice(start));
  return out;
}
