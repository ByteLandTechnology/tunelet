# Changelog

## [0.3.3](https://github.com/ByteLandTechnology/tunelet/compare/v0.3.2...v0.3.3) (2026-11-25)


### Bug Fixes

* align dry delay, stereo reverb and mono meters ([#21](https://github.com/ByteLandTechnology/tunelet/issues/21)) ([4a4110c](https://github.com/ByteLandTechnology/tunelet/commit/4a4110c3700a6df3dfafcf8230c34d5290c84308))
* **cargo:** include the README diagrams and check the crate archive ([#91](https://github.com/ByteLandTechnology/tunelet/issues/91)) ([0aff88b](https://github.com/ByteLandTechnology/tunelet/commit/0aff88b6055828b50d10c332659100d5abaadc53))
* **effects:** use per-sample envelopes and normalize ceiling ([#111](https://github.com/ByteLandTechnology/tunelet/issues/111)) ([6f0e33b](https://github.com/ByteLandTechnology/tunelet/commit/6f0e33bdcdc12aaad4b770754185f8af9cf35938))
* **resynthesis:** stop correction when a shifted period wraps ([#3](https://github.com/ByteLandTechnology/tunelet/issues/3)) ([e9f81b6](https://github.com/ByteLandTechnology/tunelet/commit/e9f81b6a73703b1d0563e25630e4417628c25c7e))
* **scale:** initialize periods for the sample rate ([#28](https://github.com/ByteLandTechnology/tunelet/issues/28)) ([95e22b9](https://github.com/ByteLandTechnology/tunelet/commit/95e22b9004d5aec021b0747af3092318446b5818))


### Performance Improvements

* **formant:** mirror histories for contiguous tap sums ([#126](https://github.com/ByteLandTechnology/tunelet/issues/126)) ([02e68e1](https://github.com/ByteLandTechnology/tunelet/commit/02e68e1fd418e700b6ef2afd508efcc930ea66c8))
* **pitch:** use bounded Q14 products in PitchTracker ([#125](https://github.com/ByteLandTechnology/tunelet/issues/125)) ([9e69547](https://github.com/ByteLandTechnology/tunelet/commit/9e69547531e1ad01f41893541e4735ada6dc3783))
* use branchless Q14 shifts and direct biquad sums ([#128](https://github.com/ByteLandTechnology/tunelet/issues/128)) ([1584500](https://github.com/ByteLandTechnology/tunelet/commit/1584500ff633675dc35a3ed3e788f35fd2e45ede))
* **wasm:** decode JS events into a reusable buffer ([#99](https://github.com/ByteLandTechnology/tunelet/issues/99)) ([f19bfe0](https://github.com/ByteLandTechnology/tunelet/commit/f19bfe098c8aefeaa4317fe7ac157e0e9c4bccc1))

## [0.3.2](https://github.com/ByteLandTechnology/tunelet/compare/v0.3.1...v0.3.2) (2026-11-21)


### Bug Fixes

* **resynthesis:** bound the pitch lag and clamp the formant period ([7eee352](https://github.com/ByteLandTechnology/tunelet/commit/7eee352a6e37913f9dcdd5d3c25676909fa62be4))
* **wasm:** remove self-references from Wasm doc comments ([4c28710](https://github.com/ByteLandTechnology/tunelet/commit/4c28710ba094f50693b54057a5b802c5fdf0c479))


### Performance Improvements

* **formant:** borrow steady banks and split ring slices ([1668f6f](https://github.com/ByteLandTechnology/tunelet/commit/1668f6f13d891fba7ffed93b13f37e8201cdc337))
* shrink formant tables and clear dirty spans on reset ([7490b83](https://github.com/ByteLandTechnology/tunelet/commit/7490b83a23705b53b60310170a1bccef86ebd58f))

## [0.3.1](https://github.com/ByteLandTechnology/tunelet/compare/v0.3.0...v0.3.1) (2026-11-21)


### Performance Improvements

* **npm:** initialize Wasm asynchronously from Node.js files ([68c01c2](https://github.com/ByteLandTechnology/tunelet/commit/68c01c2da17203f56ebb22caa5c550ac66cfc3d1))
* **pitch:** vectorize cross sums and compute shifts ([eefb67f](https://github.com/ByteLandTechnology/tunelet/commit/eefb67f3ac59de7f83e3450d3c4e6b6df9c27682))
* **resynthesis:** skip silent voice interpolation in harmony ([e56df2b](https://github.com/ByteLandTechnology/tunelet/commit/e56df2b011470354bcfab8eae21ce4fb4fb43a3a))

## [0.3.0](https://github.com/ByteLandTechnology/tunelet/compare/v0.2.0...v0.3.0) (2026-11-21)


### ⚠ BREAKING CHANGES

* **resynthesis:** harmony_gain now scales mono harmony voices.

### Features

* **resynthesis:** apply harmony_gain in mono mode ([8d29125](https://github.com/ByteLandTechnology/tunelet/commit/8d291256f6a10e10fb68e34fb0b375f2762362b6))


### Bug Fixes

* **npm:** pack the Wasm bindings and check the release archives ([b1a7790](https://github.com/ByteLandTechnology/tunelet/commit/b1a77900ce7da31f6b0b612575d9da369d5f8419))
* widen fixed-point math to i64 with saturated bounds ([6ea30d8](https://github.com/ByteLandTechnology/tunelet/commit/6ea30d834110393c5110f0c00aa1ce187a339820))


### Performance Improvements

* **cargo:** enable LTO in the release profile ([7b8e1f5](https://github.com/ByteLandTechnology/tunelet/commit/7b8e1f588882d3af4b72f94ebfebd6cdcfe29141))
* enable release LTO and Binaryen wasm-opt ([ba350a4](https://github.com/ByteLandTechnology/tunelet/commit/ba350a431a390515d2d58e28a211fea1deb19024))

## [0.2.0](https://github.com/ByteLandTechnology/tunelet/compare/v0.1.1...v0.2.0) (2026-11-11)


### Features

* **formant:** add formant envelopes for displays ([453d8d3](https://github.com/ByteLandTechnology/tunelet/commit/453d8d38e646eb256fc45d377de3c3c422ea07d8))
* **harmony:** add a voice count for the interval sources ([0bd6690](https://github.com/ByteLandTechnology/tunelet/commit/0bd66906c22a3401d688439fdc98d4ccd8bc9531))
* **processor:** add telemetry ([4a112d8](https://github.com/ByteLandTechnology/tunelet/commit/4a112d831295aaa4f3d56cff816a8e4c0d17dda5))
