/** The docs model: what `perseid-docs` builds from a spec and perseid's SDK data, and what the
 * viewer renders. Plain JSON, so a page can also fetch it. */
export interface Reference {
  title: string;
  version?: string;
  /** Markdown. */
  description?: string;
  servers: Server[];
  auth: AuthScheme[];
  /** SDK languages, in the order the switcher shows them. `curl` is always last. */
  languages: Language[];
  groups: Group[];
  /** Named schemas, by spec name. */
  models: Record<string, Model>;
  /** The requests the API sends: `webhooks` (3.1) or `x-webhooks`. */
  webhooks?: Webhook[];
}

export interface Webhook {
  /** The event, as the spec names it: `invoice.paid`. */
  name: string;
  /** URL fragment, unique in the reference. */
  slug: string;
  description?: string;
  method: string;
  /** The payload. */
  body?: Body;
}

export interface Server {
  url: string;
  description?: string;
}

export interface AuthScheme {
  id: string;
  kind: "bearer" | "basic" | "apiKey" | "oauth2" | "openIdConnect";
  /** For `apiKey`: where the key goes and its name. */
  in?: "header" | "query" | "cookie";
  name?: string;
  description?: string;
}

export interface Language {
  id: string;
  label: string;
  /** Highlighter grammar: `ts`, `python`, `go`, `bash`... */
  grammar: string;
  /** The released SDK version the names and samples match. */
  version?: string;
  install?: string;
  /** Code creating the client, shown once per page and before each sample. */
  setup?: string;
}

/** A resource of the SDKs (`customers`), or a tag of the spec without SDK data. */
export interface Group {
  id: string;
  title: string;
  description?: string;
  /** `customers.sources` under `customers`. */
  parent?: string;
  operations: Operation[];
}

export interface Operation {
  /** The operationId, or `method path` without one. */
  id: string;
  /** URL fragment, unique in the reference. */
  slug: string;
  title: string;
  description?: string;
  method: string;
  path: string;
  deprecated?: boolean;
  /** Ids of the auth schemes any one of which the operation accepts; empty when public. */
  auth: string[];
  params: Param[];
  body?: Body;
  responses: Response[];
  /** Per language id: how the SDK calls this operation. */
  sdk: Record<string, SdkCall>;
  /** Per language id, `curl` included: a complete, runnable sample. */
  samples: Record<string, string>;
  /** Pagination, streaming: notes the SDKs handle for the caller. */
  notes?: string[];
}

export interface SdkCall {
  /** `client.customers.list` */
  method: string;
  signature?: string;
  returns?: string;
  /** Parameter names in this language, by wire name. */
  params?: Record<string, string>;
}

export interface Param {
  name: string;
  in: "path" | "query" | "header" | "cookie";
  required: boolean;
  description?: string;
  deprecated?: boolean;
  schema: Schema;
}

export interface Body {
  contentType: string;
  required: boolean;
  description?: string;
  schema: Schema;
  example?: unknown;
}

export interface Response {
  status: string;
  description?: string;
  contentType?: string;
  schema?: Schema;
  example?: unknown;
}

export interface Model {
  name: string;
  description?: string;
  schema: Schema;
  /** Per language id: the type name and the property names the SDK uses. */
  sdk: Record<string, { name: string; fields?: Record<string, string> }>;
}

/** A JSON Schema reduced to what the viewer shows. Named schemas stay references. */
export interface Schema {
  ref?: string;
  type?: string | string[];
  format?: string;
  title?: string;
  description?: string;
  nullable?: boolean;
  deprecated?: boolean;
  readOnly?: boolean;
  writeOnly?: boolean;
  enum?: unknown[];
  const?: unknown;
  default?: unknown;
  example?: unknown;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  items?: Schema;
  properties?: Record<string, Schema>;
  required?: string[];
  additionalProperties?: Schema | boolean;
  oneOf?: Schema[];
  anyOf?: Schema[];
  allOf?: Schema[];
  discriminator?: { propertyName: string; mapping?: Record<string, string> };
}
