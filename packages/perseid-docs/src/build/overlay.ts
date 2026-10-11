import type { Reference } from "../reference/model.js";

/** An OpenAPI Overlay 1.0 adding each SDK's sample to every operation (`x-codeSamples`, read
 * by Scalar, Mintlify and Redocly) and its install command (`x-scalar-sdk-installation`). */
export function overlay(reference: Reference): object {
  const setup = new Map(reference.languages.map((l) => [l.id, l.setup]));
  const label = new Map(reference.languages.map((l) => [l.id, l.label]));
  const actions: object[] = [];
  actions.push({
    target: "$.info",
    update: {
      "x-scalar-sdk-installation": reference.languages
        .filter((l) => l.install)
        .map((l) => ({ lang: l.label, source: l.install })),
    },
  });
  for (const group of reference.groups) {
    for (const op of group.operations) {
      const samples = Object.entries(op.samples).map(([lang, source]) => ({
        lang: label.get(lang) ?? lang,
        label: label.get(lang) ?? lang,
        source: setup.get(lang) ? `${setup.get(lang)}\n\n${source}` : source,
      }));
      actions.push({
        target: `$.paths['${op.path.replace(/'/g, "\\'")}'].${op.method}`,
        update: { "x-codeSamples": samples },
      });
    }
  }
  return {
    overlay: "1.0.0",
    info: { title: `${reference.title} SDK samples`, version: reference.version ?? "1.0.0" },
    actions,
  };
}

/** The spec with the overlay's updates merged in, for tools that do not read overlays. */
export function applyOverlay(spec: Record<string, unknown>, reference: Reference): Record<string, unknown> {
  const out = structuredClone(spec);
  const ov = overlay(reference) as { actions: { target: string; update: object }[] };
  for (const { target, update } of ov.actions) {
    const node = target === "$.info" ? (out.info as object) : pathItem(out, target);
    if (node) Object.assign(node, update);
  }
  return out;
}

function pathItem(spec: Record<string, unknown>, target: string): object | undefined {
  const m = /^\$\.paths\['(.*)'\]\.(\w+)$/.exec(target);
  if (!m) return undefined;
  const paths = spec.paths as Record<string, Record<string, object>>;
  return paths[m[1]!.replace(/\\'/g, "'")]?.[m[2]!];
}
