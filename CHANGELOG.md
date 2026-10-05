# Changelog

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
