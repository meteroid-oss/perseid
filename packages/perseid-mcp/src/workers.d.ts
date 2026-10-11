// The Workers runtime APIs `perseid-mcp/cloudflare` uses, for compiling it outside a Worker.
declare module "cloudflare:workers" {
  export abstract class RpcTarget {}
}

// A `.wasm` import: Wrangler compiles it at deploy, a Worker receives the WebAssembly.Module.
declare module "@jitl/quickjs-wasmfile-release-sync/wasm" {
  const module: WebAssembly.Module;
  export default module;
}
