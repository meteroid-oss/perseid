# Native binary releases

Move `release-binaries.yml` to `.github/workflows/release-binaries.yml` once.
It is staged here because the current contributor credential cannot write active
GitHub workflows. Consumer workflows are embedded in the binary and are installed
by `perseid init`; they need no such manual copy.

Keep Cargo.toml and Cargo.lock versions in sync, merge the release, then push the
matching `vX.Y.Z` tag. The workflow tests/builds Linux (static musl) and macOS binaries
for x86_64 and arm64, smoke-tests init, and publishes all four archives plus
SHA256SUMS together. A manual run can resume an unpublished/draft release for an
existing tag. Published assets cannot be replaced by this workflow; make a new
version for fixes. Generated CI downloads the exact version that created it.

The first binary release must exist before consumer CI can run. Release artifacts
include all templates and runtimes; no checkout, Cargo, or Python is required to
bootstrap. macOS binaries are not notarized. Windows binaries are not shipped yet.
The existing Dockerfile remains the option for a generator with bundled formatters;
this workflow publishes native binaries only.
