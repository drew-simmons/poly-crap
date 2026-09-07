# Changelog

## [0.8.1](https://github.com/drew-simmons/poly-crap/compare/v0.8.0...v0.8.1) (2026-09-06)


### Bug Fixes

* correct the skill's curl install path ([#38](https://github.com/drew-simmons/poly-crap/issues/38)) ([6ee6961](https://github.com/drew-simmons/poly-crap/commit/6ee6961575a42735c33817e0ee3ad457219d23f3))

## [0.8.0](https://github.com/drew-simmons/poly-crap/compare/v0.7.0...v0.8.0) (2026-09-03)


### Features

* color and align human output ([#35](https://github.com/drew-simmons/poly-crap/issues/35)) ([87cbfd9](https://github.com/drew-simmons/poly-crap/commit/87cbfd99d3dc1b2095dec0c4fb0a5f121d3b7f1d))

## [0.7.0](https://github.com/drew-simmons/poly-crap/compare/v0.6.0...v0.7.0) (2026-09-03)


### ⚠ BREAKING CHANGES

* tighten complexity rules, qualify more symbols, and read Cobertura reports ([#33](https://github.com/drew-simmons/poly-crap/issues/33))

### Features

* list only failing functions in human output and name their uncovered lines ([#27](https://github.com/drew-simmons/poly-crap/issues/27)) ([254ad1c](https://github.com/drew-simmons/poly-crap/commit/254ad1cd417bbd22f074386f0cc63c7a1eb6af49))
* tighten complexity rules, qualify more symbols, and read Cobertura reports ([#33](https://github.com/drew-simmons/poly-crap/issues/33)) ([a0181a2](https://github.com/drew-simmons/poly-crap/commit/a0181a2ad7a0e00c356c5e0b9f5fbf1c6fd373d6))
* warn when a coverage report is older than the source it covers ([#32](https://github.com/drew-simmons/poly-crap/issues/32)) ([9a94982](https://github.com/drew-simmons/poly-crap/commit/9a9498274dbc1ada761f255a4211300324599e26))


### Bug Fixes

* gate baseline runs on --fail-above and reject file-name-only coverage matches ([#26](https://github.com/drew-simmons/poly-crap/issues/26)) ([46ed871](https://github.com/drew-simmons/poly-crap/commit/46ed8719036ec12b4b1e8816cb3c3c1d4f7f2a40))
* stop reporting a line shift within a file as a moved function ([#29](https://github.com/drew-simmons/poly-crap/issues/29)) ([21ed551](https://github.com/drew-simmons/poly-crap/commit/21ed5518141860e2bd934c55a32618ba24a57f51))

## [0.6.0](https://github.com/drew-simmons/poly-crap/compare/v0.5.1...v0.6.0) (2026-08-24)


### Features

* auto-discover coverage reports at default locations ([#21](https://github.com/drew-simmons/poly-crap/issues/21)) ([c74bef2](https://github.com/drew-simmons/poly-crap/commit/c74bef23aecb177acde47b1f48e0364d6c1268d2))

## [0.5.1](https://github.com/drew-simmons/poly-crap/compare/v0.5.0...v0.5.1) (2026-08-21)


### Bug Fixes

* cargo release ([#19](https://github.com/drew-simmons/poly-crap/issues/19)) ([07da640](https://github.com/drew-simmons/poly-crap/commit/07da6407bb57e3f7a2bbe5bd07fb888f13f6e52d))

## [0.5.0](https://github.com/drew-simmons/poly-crap/compare/v0.4.0...v0.5.0) (2026-08-21)


### Features

* dogfood poly-crap ([#17](https://github.com/drew-simmons/poly-crap/issues/17)) ([f3bc06a](https://github.com/drew-simmons/poly-crap/commit/f3bc06afc6ad0325250bce0671b779469b6be097))

## [0.4.0](https://github.com/drew-simmons/poly-crap/compare/v0.3.0...v0.4.0) (2026-08-21)


### Features

* add poly-crap skill ([#15](https://github.com/drew-simmons/poly-crap/issues/15)) ([70dce81](https://github.com/drew-simmons/poly-crap/commit/70dce81ec67d26b912cb174633909f13d25999b2))

## [0.3.0](https://github.com/drew-simmons/poly-crap/compare/v0.2.0...v0.3.0) (2026-08-20)


### ⚠ BREAKING CHANGES

* remove Terraform support ([#10](https://github.com/drew-simmons/poly-crap/issues/10))

### Features

* remove Terraform support ([#10](https://github.com/drew-simmons/poly-crap/issues/10)) ([225c7fa](https://github.com/drew-simmons/poly-crap/commit/225c7fa60b78009c3d3576ca460088a2d4659e14))


### Bug Fixes

* correct allow globs, baseline gating, and Git diff scoping ([#11](https://github.com/drew-simmons/poly-crap/issues/11)) ([256ae91](https://github.com/drew-simmons/poly-crap/commit/256ae91e8a08d749c5e6f2297a449aeb338ade75))

## [0.2.0](https://github.com/drew-simmons/poly-crap/compare/v0.1.1...v0.2.0) (2026-08-14)


### Features

* add Git diff checks for changed functions and Terraform blocks ([#8](https://github.com/drew-simmons/poly-crap/issues/8)) ([fad4d50](https://github.com/drew-simmons/poly-crap/commit/fad4d5024d99f5215b1a9a6342c4502f68fe5926))

## [0.1.1](https://github.com/drew-simmons/poly-crap/compare/v0.1.0...v0.1.1) (2026-08-13)


### Bug Fixes

* release workflow ([de071cb](https://github.com/drew-simmons/poly-crap/commit/de071cb9be8636280ed5b11bb250a9d5ccff5a67))

## 0.1.0 (2026-08-13)


### Features

* initial commit ([810f548](https://github.com/drew-simmons/poly-crap/commit/810f548e49e687a947c581ca737f1bd0aba41812))
* initial v1 crap of core language ([#2](https://github.com/drew-simmons/poly-crap/issues/2)) ([c7bfbfe](https://github.com/drew-simmons/poly-crap/commit/c7bfbfe2861573564e28b22a9c086fa4c3c2ce16))


### Bug Fixes

* cargo release ([2390cdc](https://github.com/drew-simmons/poly-crap/commit/2390cdc981aba5b4b19848760ac8ad168348667d))
* missing cargo lock ([d1b4071](https://github.com/drew-simmons/poly-crap/commit/d1b4071ae81873722423871edf2ef87fd3f654db))
* release token ([94b3f57](https://github.com/drew-simmons/poly-crap/commit/94b3f57ea87bda6e68873bcd0ab3aef08144d942))

## Changelog

All notable changes to this project are documented in this file. Entries and
versions are generated by Release Please from Conventional Commits.
