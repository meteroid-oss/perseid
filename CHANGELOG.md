# Changelog

## [0.13.0](https://github.com/meteroid-oss/perseid/compare/v0.12.1...v0.13.0) (2026-10-08)


### ⚠ BREAKING CHANGES

* **api:** SDK call paths move into child resources: `client.workspaces.listPeers()` becomes `client.workspaces.peers.list()`. A collection-level PUT or DELETE next to an item-level one is named `set` or `delete_all`. Pin a method with `[resources]` or `x-perseid-resource` to keep its call path.
* query parameters that are unions with an object variant, such as Stripe's `created`, change type in the generated SDKs: Go's `json.RawMessage` becomes a union type (`*WireSearchCreated`), Java's `Object` a union class of the options (`WireSearchOptions.Created`), and TypeScript's `unknown` the union of its variants. In Rust, the model of the object variant (`RangeQuerySpecs`) is a request model: no longer `#[non_exhaustive]`, with a setter per field.

### Features

* **api:** nested resources from the paths within each tag ([#108](https://github.com/meteroid-oss/perseid/issues/108)) ([1e76db1](https://github.com/meteroid-oss/perseid/commit/1e76db1761219ea062e48477c88ad9a29548a98c))
* **docs:** examples with list and object arguments ([#105](https://github.com/meteroid-oss/perseid/issues/105)) ([c012367](https://github.com/meteroid-oss/perseid/commit/c012367fff509c70291bfcdcf2de89d6ea495b92))
* **init:** import a Stainless config with --from ([#101](https://github.com/meteroid-oss/perseid/issues/101)) ([2ea8874](https://github.com/meteroid-oss/perseid/commit/2ea8874cc123e42e0fc5b36fe11cb21fae3747b9))
* perseid docs-data, each SDK's names and calls for docs sites ([#106](https://github.com/meteroid-oss/perseid/issues/106)) ([989fcbe](https://github.com/meteroid-oss/perseid/commit/989fcbe61a8eac1f6af7dee3491bbe60d3913c9e))
* **spec:** read Swagger 2.0 specs directly ([#100](https://github.com/meteroid-oss/perseid/issues/100)) ([262bd53](https://github.com/meteroid-oss/perseid/commit/262bd5341d81fb702b53cd69cf4d3d8cdfe978e2))
* Stripe-style pagination, typed query unions and Azure discriminator hierarchies ([#107](https://github.com/meteroid-oss/perseid/issues/107)) ([467dd22](https://github.com/meteroid-oss/perseid/commit/467dd22b127746f84bded1b89eeb69ec80775c3f))

## [0.12.1](https://github.com/meteroid-oss/perseid/compare/v0.12.0...v0.12.1) (2026-10-07)


### Bug Fixes

* **security:** check only generated operations, drop undeclared alternatives ([#97](https://github.com/meteroid-oss/perseid/issues/97)) ([31c9cae](https://github.com/meteroid-oss/perseid/commit/31c9cae5c8625e82ce5fbe32ed0217e8584f3c79))

## [0.12.0](https://github.com/meteroid-oss/perseid/compare/v0.11.4...v0.12.0) (2026-10-07)


### ⚠ BREAKING CHANGES

* opt-in idempotency keys; Rust ID newtypes, setters and explicit from_env ([#95](https://github.com/meteroid-oss/perseid/issues/95))

### Features

* opt-in idempotency keys; Rust ID newtypes, setters and explicit from_env ([#95](https://github.com/meteroid-oss/perseid/issues/95)) ([5effc25](https://github.com/meteroid-oss/perseid/commit/5effc259a7ebddb6618a1665186eaddd85534797))
* **pagination:** a list page is the response body, one list() entry point ([#96](https://github.com/meteroid-oss/perseid/issues/96)) ([da2bc3f](https://github.com/meteroid-oss/perseid/commit/da2bc3f98cb856f0e724f74f959b6334446c6263))


### Bug Fixes

* **pr:** one update pull request per SDK when an update is too large for release-please ([#91](https://github.com/meteroid-oss/perseid/issues/91)) ([749bad8](https://github.com/meteroid-oss/perseid/commit/749bad814b87db68d5f68d2a82d9121bddd4d5aa))
* strict response decoding in TypeScript and Go, reject undeclared security schemes ([#92](https://github.com/meteroid-oss/perseid/issues/92)) ([d52dc7c](https://github.com/meteroid-oss/perseid/commit/d52dc7c41363e906e1dab05b27f782e9f560c1ea))

## [0.11.4](https://github.com/meteroid-oss/perseid/compare/v0.11.3...v0.11.4) (2026-10-06)


### Features

* **config:** auto_merge writes auto-merge into sdks.yml ([#89](https://github.com/meteroid-oss/perseid/issues/89)) ([b86df32](https://github.com/meteroid-oss/perseid/commit/b86df324ce2f3c18f5ef2ed9efe54450c9ddfb8f))

## [0.11.3](https://github.com/meteroid-oss/perseid/compare/v0.11.2...v0.11.3) (2026-10-06)


### Features

* **publish:** NuGet trusted publishing ([#87](https://github.com/meteroid-oss/perseid/issues/87)) ([da8b65c](https://github.com/meteroid-oss/perseid/commit/da8b65c3267e487b5455164734661eef9e930524))


### Bug Fixes

* list only each repository's SDKs in its pull request, credit perseid at the bottom ([#86](https://github.com/meteroid-oss/perseid/issues/86)) ([e03b7d5](https://github.com/meteroid-oss/perseid/commit/e03b7d510cea8a73ee0475f72356c771611579d3))

## [0.11.2](https://github.com/meteroid-oss/perseid/compare/v0.11.1...v0.11.2) (2026-10-06)


### Bug Fixes

* **init:** suggest public SDK repositories in one copyable command ([#83](https://github.com/meteroid-oss/perseid/issues/83)) ([d6602a5](https://github.com/meteroid-oss/perseid/commit/d6602a53973c84efa009f759b59dfd0ffef86ba9))
* keep the generated workflow headers to one line ([#84](https://github.com/meteroid-oss/perseid/issues/84)) ([93226c4](https://github.com/meteroid-oss/perseid/commit/93226c475b4086a9a1cd23f8eb2c1718a23f77be))
* pypi token ([69b3960](https://github.com/meteroid-oss/perseid/commit/69b3960482243740230b3852ba5444605f2a6c04))

## [0.11.1](https://github.com/meteroid-oss/perseid/compare/v0.11.0...v0.11.1) (2026-10-06)


### Bug Fixes

* keep the Rust crate name as written, run workflows on origin/main without origin/HEAD ([#81](https://github.com/meteroid-oss/perseid/issues/81)) ([3c16174](https://github.com/meteroid-oss/perseid/commit/3c16174b55a7de6e2d6e49d00935caf7a6e0cbc0))

## [0.11.0](https://github.com/meteroid-oss/perseid/compare/v0.10.0...v0.11.0) (2026-10-06)


### ⚠ BREAKING CHANGES

* infer pagination items and use stop values only where responses have them ([#79](https://github.com/meteroid-oss/perseid/issues/79))

### Features

* infer pagination items and use stop values only where responses have them ([#79](https://github.com/meteroid-oss/perseid/issues/79)) ([108a3aa](https://github.com/meteroid-oss/perseid/commit/108a3aa8c94ad69aca00ac52dedb0b14e9217f9c))

## [0.10.0](https://github.com/meteroid-oss/perseid/compare/v0.9.0...v0.10.0) (2026-10-06)


### ⚠ BREAKING CHANGES

* type values as precisely as the spec does, in every SDK ([#77](https://github.com/meteroid-oss/perseid/issues/77))

### Features

* opt-in round trips of every model in the generated tests ([#78](https://github.com/meteroid-oss/perseid/issues/78)) ([bfd4956](https://github.com/meteroid-oss/perseid/commit/bfd4956d0221be958571346c9324c68ec1df4ee9))
* test the SDKs in CI, and ask for their license on init ([#75](https://github.com/meteroid-oss/perseid/issues/75)) ([01aa85e](https://github.com/meteroid-oss/perseid/commit/01aa85e2d638d795a98ec26851371ce4b7f6c181))
* type values as precisely as the spec does, in every SDK ([#77](https://github.com/meteroid-oss/perseid/issues/77)) ([a407863](https://github.com/meteroid-oss/perseid/commit/a407863bbbf544e08a52dfce8c40cb98f0fccf0c))

## [0.9.0](https://github.com/meteroid-oss/perseid/compare/v0.8.0...v0.9.0) (2026-10-06)


### ⚠ BREAKING CHANGES

* SDK pull requests no longer add or update sdk-release.yml: run perseid sync. connect defaults to the perseid App.

### Features

* hosted perseid App ([#73](https://github.com/meteroid-oss/perseid/issues/73)) ([7c1b93f](https://github.com/meteroid-oss/perseid/commit/7c1b93ffde5b240abe6f959245c52c4b2ebf123f))

## [0.8.0](https://github.com/meteroid-oss/perseid/compare/v0.7.2...v0.8.0) (2026-10-05)


### ⚠ BREAKING CHANGES

* **connect:** push the spec as the SDKs' GitHub App or with a token ([#71](https://github.com/meteroid-oss/perseid/issues/71))

### Features

* **connect:** push the spec as the SDKs' GitHub App or with a token ([#71](https://github.com/meteroid-oss/perseid/issues/71)) ([482708a](https://github.com/meteroid-oss/perseid/commit/482708a17cce89e2a469d44dd38b1d3b4c352b8c))


### Bug Fixes

* **init:** ask for the API name in any case, checked before anything is written ([#70](https://github.com/meteroid-oss/perseid/issues/70)) ([409faa5](https://github.com/meteroid-oss/perseid/commit/409faa5e9e202fb879ee7ff24aafa5f5b10a63da))

## [0.7.2](https://github.com/meteroid-oss/perseid/compare/v0.7.1...v0.7.2) (2026-10-05)


### Features

* Improve init flow and next steps messaging ([#68](https://github.com/meteroid-oss/perseid/issues/68)) ([bcd3e83](https://github.com/meteroid-oss/perseid/commit/bcd3e838540f27daa32246f0bb2a2e0d070cf79d))

## [0.7.1](https://github.com/meteroid-oss/perseid/compare/v0.7.0...v0.7.1) (2026-10-03)


### Bug Fixes

* review findings on spec bundling, unions, OAuth and wire encoding ([#64](https://github.com/meteroid-oss/perseid/issues/64)) ([75f14c0](https://github.com/meteroid-oss/perseid/commit/75f14c0d2b5be8f784dc193176e6ac3b693f71ec))

## [0.7.0](https://github.com/meteroid-oss/perseid/compare/v0.6.0...v0.7.0) (2026-10-02)


### ⚠ BREAKING CHANGES

* Full-featured SDKs in all six languages ([#61](https://github.com/meteroid-oss/perseid/issues/61))

### Features

* Full-featured SDKs in all six languages ([#61](https://github.com/meteroid-oss/perseid/issues/61)) ([e6cfc1e](https://github.com/meteroid-oss/perseid/commit/e6cfc1ea8e95e690a8aa545ed86f9a25174c2188))
* support parameter styles and JSON content parameters, skip unsupported operations ([4132afc](https://github.com/meteroid-oss/perseid/commit/4132afc4dde12f385769c1bc71552741923ce058))

## [0.6.0](https://github.com/meteroid-oss/perseid/compare/v0.5.1...v0.6.0) (2026-10-01)


### ⚠ BREAKING CHANGES

* the action's logic moves into the binary; sdks.yml hands the App credentials to the action ([#54](https://github.com/meteroid-oss/perseid/issues/54))
* rename [package] to [metadata] and names to methods in perseid.toml ([#53](https://github.com/meteroid-oss/perseid/issues/53))
* init writes the workflows locally, setup becomes setup-github ([#52](https://github.com/meteroid-oss/perseid/issues/52))
* perseid-push.yml calls `perseid push-spec`; connect writes it locally, with --auth ([#43](https://github.com/meteroid-oss/perseid/issues/43))

### Features

* init writes the workflows locally, setup becomes setup-github ([#52](https://github.com/meteroid-oss/perseid/issues/52)) ([c388cd8](https://github.com/meteroid-oss/perseid/commit/c388cd89d7b8599f7afb6e65fcae7287be89f482))
* meteroid-oss/perseid/release action, shrinking sdk-release.yml ([#56](https://github.com/meteroid-oss/perseid/issues/56)) ([418eed3](https://github.com/meteroid-oss/perseid/commit/418eed3bfa16b8171670960104d5e313d90d7d7a))
* perseid-push.yml calls `perseid push-spec`; connect writes it locally, with --auth ([#43](https://github.com/meteroid-oss/perseid/issues/43)) ([0159ea2](https://github.com/meteroid-oss/perseid/commit/0159ea217307b8ee5ee36811883d7f76cd0996b2))
* rename [package] to [metadata] and names to methods in perseid.toml ([#53](https://github.com/meteroid-oss/perseid/issues/53)) ([13eb21e](https://github.com/meteroid-oss/perseid/commit/13eb21e817136e1ae54ec9b255603186ab91062a))
* the action's logic moves into the binary; sdks.yml hands the App credentials to the action ([#54](https://github.com/meteroid-oss/perseid/issues/54)) ([2dfabf4](https://github.com/meteroid-oss/perseid/commit/2dfabf427fa7e3a467d27acdb166c586ed822e43))


### Bug Fixes

* commit generate --pr from a temporary worktree ([#48](https://github.com/meteroid-oss/perseid/issues/48)) ([b1946e0](https://github.com/meteroid-oss/perseid/commit/b1946e07039d5a030b01b16eec87c5174df6a2bf))
* **generate:** one rule for local SDKs, --out preview, guarded checkouts ([#50](https://github.com/meteroid-oss/perseid/issues/50)) ([5f9bdbc](https://github.com/meteroid-oss/perseid/commit/5f9bdbcff676185c034b7032d23bbb667603fe8a))
* init keeps going on bad specs, Java package from the homepage, inspect/eject per language ([#55](https://github.com/meteroid-oss/perseid/issues/55)) ([fe83ac2](https://github.com/meteroid-oss/perseid/commit/fe83ac2e347be4502016a45619846c3c71ea3016))
* name each SDK's own repository in its manifests ([#47](https://github.com/meteroid-oss/perseid/issues/47)) ([e2decfa](https://github.com/meteroid-oss/perseid/commit/e2decfa8b3f2b725283147017384d92495945775))
* pin generated workflows to the release line, not [@v0](https://github.com/v0) ([#51](https://github.com/meteroid-oss/perseid/issues/51)) ([974a104](https://github.com/meteroid-oss/perseid/commit/974a10414df7a49b87a3419185d2a652a985fe73))
* **push-spec:** fail on diverged history instead of skipping every later push ([#46](https://github.com/meteroid-oss/perseid/issues/46)) ([47ddeb8](https://github.com/meteroid-oss/perseid/commit/47ddeb8a112b067abd1d399df2d36621518d2360))
* **release:** sdk-release.yml template error, default branch, root placement, publish hardening ([#49](https://github.com/meteroid-oss/perseid/issues/49)) ([cac0ec6](https://github.com/meteroid-oss/perseid/commit/cac0ec6b9a0460936ed1e5e94d50b44541179e6d))

## [0.5.1](https://github.com/meteroid-oss/perseid/compare/v0.5.0...v0.5.1) (2026-10-01)


### Features

* arrow-key and checkbox prompts for init, setup and connect ([#41](https://github.com/meteroid-oss/perseid/issues/41)) ([c57f5a8](https://github.com/meteroid-oss/perseid/commit/c57f5a8bdeb212aadc7ac1703367c16cd8d4055f))

## [0.5.0](https://github.com/meteroid-oss/perseid/compare/v0.4.0...v0.5.0) (2026-10-01)


### ⚠ BREAKING CHANGES

* init writes perseid.toml only, `perseid connect` pushes the spec ([#39](https://github.com/meteroid-oss/perseid/issues/39))

### Features

* init writes perseid.toml only, `perseid connect` pushes the spec ([#39](https://github.com/meteroid-oss/perseid/issues/39)) ([d7d9f74](https://github.com/meteroid-oss/perseid/commit/d7d9f7412f8474b08e1d3ab7a6fc5694aa16c6b7))

## [0.4.0](https://github.com/meteroid-oss/perseid/compare/v0.3.2...v0.4.0) (2026-10-01)


### ⚠ BREAKING CHANGES

* `sdks_repo`, `push_on`, `push_tags` and `generate` moved to `[push]` as `to`, `on`, `tags` and `generate`.
* `push_spec` is now `sdks_repo`; top-level `description`, `license`, `homepage`, `repository` and `authors` moved to `[package]`; `include` was removed (`internal = true`, or `only`); `version` was removed; `flat_unions` moved from `[python.context]` to `[python]`; `int64` outside `[typescript]` and `flat_unions` outside `[python]` are rejected.
* removed `init --github` and the `method_names`, `typed_unions`, `patch_nullable`, `initialisms` and `edition` keys; Java edition 1 output and scaffold compatibility shims are gone.

### Features

* declarative repository layouts, perseid setup and perseid status ([#33](https://github.com/meteroid-oss/perseid/issues/33)) ([d4a5818](https://github.com/meteroid-oss/perseid/commit/d4a58188fb3e11a5d178a91216fbfff36152cd86))
* group spec-push settings in a [push] table ([#37](https://github.com/meteroid-oss/perseid/issues/37)) ([8d2c487](https://github.com/meteroid-oss/perseid/commit/8d2c487275c45d0641314040e53e05eff99da705))
* perseid.toml [package] table, sdks_repo with push_on, simpler filters and a JSON Schema ([#36](https://github.com/meteroid-oss/perseid/issues/36)) ([92d3506](https://github.com/meteroid-oss/perseid/commit/92d3506cba9bcdc89eda2a022b00d07146c742d7))


### Code Refactoring

* drop compatibility switches and deprecated commands ([#35](https://github.com/meteroid-oss/perseid/issues/35)) ([5c4ec8b](https://github.com/meteroid-oss/perseid/commit/5c4ec8b2ac9c73498478ce72208cb2a49c168339))

## [0.3.2](https://github.com/meteroid-oss/perseid/compare/v0.3.1...v0.3.2) (2026-09-30)


### Features

* perseid init --github ([#32](https://github.com/meteroid-oss/perseid/issues/32)) ([2c18728](https://github.com/meteroid-oss/perseid/commit/2c187284e02e17ee2495859646e69c03942bb6c8))
* publish perseid to npm ([#31](https://github.com/meteroid-oss/perseid/issues/31)) ([8858dbf](https://github.com/meteroid-oss/perseid/commit/8858dbfc718f8ab4f14afa870cf6345f18c5810e))
* type unions of objects without a discriminator ([#29](https://github.com/meteroid-oss/perseid/issues/29)) ([ea63451](https://github.com/meteroid-oss/perseid/commit/ea634511f17cc08330fba70c42fea281fec323d6))

## [0.3.1](https://github.com/meteroid-oss/perseid/compare/v0.3.0...v0.3.1) (2026-09-30)


### Features

* resource method names, typed errors, expandable unions and per-language ergonomics ([#28](https://github.com/meteroid-oss/perseid/issues/28)) ([e537824](https://github.com/meteroid-oss/perseid/commit/e53782473b9ae66203912606f162674f532e18c7))
* token options for generated and release PRs ([#26](https://github.com/meteroid-oss/perseid/issues/26)) ([f10d6df](https://github.com/meteroid-oss/perseid/commit/f10d6df11b25064ad2b7e75a27b65cbb83e7fcd6))
