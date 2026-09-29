export class ApiException extends Error {
  public readonly headers: Record<string, string> = {};
  constructor(public readonly status: number, public readonly body: string, headers: Headers) {
    super(`API error ${status}: ${body}`);
    this.name = "ApiException";
    headers.forEach((value, key) => { this.headers[key] = value; });
  }
}
export type XOR<T, U> = (T & { [K in keyof U]?: never }) | (U & { [K in keyof T]?: never });
