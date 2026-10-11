/** A Shiki theme, as far as the light variant below touches it. */
export interface ShikiTheme {
  name?: string;
  tokenColors?: { scope?: string | string[]; settings: Record<string, string> }[];
  [key: string]: unknown;
}

/** GitHub Light with the values told apart from the keys, the way the dark theme does: strings
 * green, numbers orange, literals purple. Every color passes 4.5:1 on white. */
export function lightTheme<T extends object>(theme: T): T {
  const githubLight = theme as ShikiTheme;
  return {
    ...githubLight,
    name: "perseid-light",
    tokenColors: [
      ...(githubLight.tokenColors ?? []),
      { scope: ["string", "string.quoted", "punctuation.definition.string"], settings: { foreground: "#116329" } },
      { scope: ["constant.numeric"], settings: { foreground: "#953800" } },
      { scope: ["constant.language"], settings: { foreground: "#8250df" } },
      { scope: ["support.type.property-name", "support.type.property-name.json"], settings: { foreground: "#0550ae" } },
    ],
  } as unknown as T;
}
