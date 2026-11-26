/** What `perseid docs-data` writes: how each SDK names and calls the API. */
export interface DocsData {
  name?: string;
  base_url?: string | null;
  resources?: DataResource[];
  languages: Record<string, SdkData>;
}

export interface DataResource {
  name: string;
  /** Operation ids, in order. */
  operations: string[];
  subresources?: DataResource[];
}

/** One SDK's names and calls. */
export interface SdkData {
  /** The released SDK version this data matches, when the orchestrator knows it. */
  version?: string | null;
  install: string;
  setup: string;
  operations: Record<
    string,
    { method: string; signature: string; sample: string; complete: boolean; params: Record<string, string> }
  >;
  types: Record<string, { name: string; fields: Record<string, string> }>;
}
