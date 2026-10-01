# Changelog

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
