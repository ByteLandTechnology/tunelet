# Changelog

## [0.4.0](https://github.com/ByteLandTechnology/tunelet/compare/v0.3.4...v0.4.0) (2026-11-27)


### ⚠ BREAKING CHANGES

* **events:** add scalar parameter control events ([#61](https://github.com/ByteLandTechnology/tunelet/issues/61))
* add pitch fields to Telemetry and trim Wasm voice slices ([#104](https://github.com/ByteLandTechnology/tunelet/issues/104))
* **resynthesis:** harmony_gain now scales mono harmony voices.

### Features

* add control activity queries ([#135](https://github.com/ByteLandTechnology/tunelet/issues/135)) ([655b7fd](https://github.com/ByteLandTechnology/tunelet/commit/655b7fd8f8dc21d12a0cca088fc4e7543d0da6b4))
* add pitch fields to Telemetry and trim Wasm voice slices ([#104](https://github.com/ByteLandTechnology/tunelet/issues/104)) ([66d372b](https://github.com/ByteLandTechnology/tunelet/commit/66d372b309cf5a3db46c143a1c493b1a4c377b95))
* **effects:** add the effects chain ([de6eda4](https://github.com/ByteLandTechnology/tunelet/commit/de6eda4a3bc39487b5b106110d3d6ecf0a1035f5))
* **events:** add scalar parameter control events ([#61](https://github.com/ByteLandTechnology/tunelet/issues/61)) ([8880205](https://github.com/ByteLandTechnology/tunelet/commit/8880205ca2a82359ccbd2d577ad788c8d543f9a0))
* expose Float32 buffers in Wasm for in-place audio ([#117](https://github.com/ByteLandTechnology/tunelet/issues/117)) ([4807598](https://github.com/ByteLandTechnology/tunelet/commit/48075988e82b753b96457fd8683ac5c44291446d))
* **formant:** add formant envelopes for displays ([453d8d3](https://github.com/ByteLandTechnology/tunelet/commit/453d8d38e646eb256fc45d377de3c3c422ea07d8))
* **harmony:** add a voice count for the interval sources ([0bd6690](https://github.com/ByteLandTechnology/tunelet/commit/0bd66906c22a3401d688439fdc98d4ccd8bc9531))
* **harmony:** add the harmony voices and the chords ([fae3645](https://github.com/ByteLandTechnology/tunelet/commit/fae3645209a665c056b893eb34c01f9ac331adc9))
* **npm:** add the TypeScript package and the README ([9ec0e2a](https://github.com/ByteLandTechnology/tunelet/commit/9ec0e2a6546b3ae950f41bc9a71e7bbbfcdb4734))
* **numeric:** add fixed-point arithmetic and error types ([8df523b](https://github.com/ByteLandTechnology/tunelet/commit/8df523b6d49992224e00fa30e5975426ff7db2aa))
* **pitch:** add the pitch tracker, the scales and MIDI input ([9bcf77f](https://github.com/ByteLandTechnology/tunelet/commit/9bcf77f5044bbc1dd8bac270431a13132aed3d76))
* **processor:** add telemetry ([4a112d8](https://github.com/ByteLandTechnology/tunelet/commit/4a112d831295aaa4f3d56cff816a8e4c0d17dda5))
* **processor:** add the processor and the public API ([0431ad2](https://github.com/ByteLandTechnology/tunelet/commit/0431ad2dd7f585eafcb4a293333cfc02953277ca))
* **resynthesis:** add humanize and flex-tune controls ([#45](https://github.com/ByteLandTechnology/tunelet/issues/45)) ([0850e25](https://github.com/ByteLandTechnology/tunelet/commit/0850e25f972084a0a9e03cda37dec44f40dc146e))
* **resynthesis:** add LPC analysis, formant control and resynthesis ([53e42b0](https://github.com/ByteLandTechnology/tunelet/commit/53e42b00e0c0cafe83e6c90c74215c8126e0a6b7))
* **resynthesis:** apply harmony_gain in mono mode ([8d29125](https://github.com/ByteLandTechnology/tunelet/commit/8d291256f6a10e10fb68e34fb0b375f2762362b6))
* **wasm:** add the optional Wasm bindings ([8e9456c](https://github.com/ByteLandTechnology/tunelet/commit/8e9456c16e6d2ccb16c2c1ee2906d64bcff1b8fc))


### Bug Fixes

* align dry delay, stereo reverb and mono meters ([#21](https://github.com/ByteLandTechnology/tunelet/issues/21)) ([4a4110c](https://github.com/ByteLandTechnology/tunelet/commit/4a4110c3700a6df3dfafcf8230c34d5290c84308))
* **cargo:** include the README diagrams and check the crate archive ([#91](https://github.com/ByteLandTechnology/tunelet/issues/91)) ([0aff88b](https://github.com/ByteLandTechnology/tunelet/commit/0aff88b6055828b50d10c332659100d5abaadc53))
* **effects:** use per-sample envelopes and normalize ceiling ([#111](https://github.com/ByteLandTechnology/tunelet/issues/111)) ([6f0e33b](https://github.com/ByteLandTechnology/tunelet/commit/6f0e33bdcdc12aaad4b770754185f8af9cf35938))
* **midi:** reset harmony on source change and fix correction keys ([#46](https://github.com/ByteLandTechnology/tunelet/issues/46)) ([7dea4b3](https://github.com/ByteLandTechnology/tunelet/commit/7dea4b366f13101571c271969076b86276c807b1))
* **npm:** normalize JSDoc comments at the package boundary ([#107](https://github.com/ByteLandTechnology/tunelet/issues/107)) ([bd9ef47](https://github.com/ByteLandTechnology/tunelet/commit/bd9ef4713dbe4cefcdb421c4bf041966e0051c75))
* **npm:** pack the Wasm bindings and check the release archives ([b1a7790](https://github.com/ByteLandTechnology/tunelet/commit/b1a77900ce7da31f6b0b612575d9da369d5f8419))
* **resynthesis:** bound the pitch lag and clamp the formant period ([7eee352](https://github.com/ByteLandTechnology/tunelet/commit/7eee352a6e37913f9dcdd5d3c25676909fa62be4))
* **resynthesis:** stop correction when a shifted period wraps ([#3](https://github.com/ByteLandTechnology/tunelet/issues/3)) ([e9f81b6](https://github.com/ByteLandTechnology/tunelet/commit/e9f81b6a73703b1d0563e25630e4417628c25c7e))
* **scale:** initialize periods for the sample rate ([#28](https://github.com/ByteLandTechnology/tunelet/issues/28)) ([95e22b9](https://github.com/ByteLandTechnology/tunelet/commit/95e22b9004d5aec021b0747af3092318446b5818))
* **wasm:** remove self-references from Wasm doc comments ([4c28710](https://github.com/ByteLandTechnology/tunelet/commit/4c28710ba094f50693b54057a5b802c5fdf0c479))
* **wasm:** sanitize TypeScript declarations and Wasm exports ([#123](https://github.com/ByteLandTechnology/tunelet/issues/123)) ([254ce7d](https://github.com/ByteLandTechnology/tunelet/commit/254ce7d9cbacfaa8eff770c653e3401a847a7847))
* widen fixed-point math to i64 with saturated bounds ([6ea30d8](https://github.com/ByteLandTechnology/tunelet/commit/6ea30d834110393c5110f0c00aa1ce187a339820))


### Performance Improvements

* cache Warm reciprocals and accelerate AtanQ14 ([#113](https://github.com/ByteLandTechnology/tunelet/issues/113)) ([7ae0f23](https://github.com/ByteLandTechnology/tunelet/commit/7ae0f2332787399a6ae9870a6d07f210ee525922))
* **cargo:** enable LTO in the release profile ([7b8e1f5](https://github.com/ByteLandTechnology/tunelet/commit/7b8e1f588882d3af4b72f94ebfebd6cdcfe29141))
* enable release LTO and Binaryen wasm-opt ([ba350a4](https://github.com/ByteLandTechnology/tunelet/commit/ba350a431a390515d2d58e28a211fea1deb19024))
* **formant:** borrow steady banks and split ring slices ([1668f6f](https://github.com/ByteLandTechnology/tunelet/commit/1668f6f13d891fba7ffed93b13f37e8201cdc337))
* **formant:** fuse envelope rotations and cache Wasm results ([#30](https://github.com/ByteLandTechnology/tunelet/issues/30)) ([c815dd5](https://github.com/ByteLandTechnology/tunelet/commit/c815dd5dfb6d6aa41dd1df4e7dc7c7b2d7ed324b))
* **formant:** mirror histories for contiguous tap sums ([#126](https://github.com/ByteLandTechnology/tunelet/issues/126)) ([02e68e1](https://github.com/ByteLandTechnology/tunelet/commit/02e68e1fd418e700b6ef2afd508efcc930ea66c8))
* **npm:** initialize Wasm asynchronously from Node.js files ([68c01c2](https://github.com/ByteLandTechnology/tunelet/commit/68c01c2da17203f56ebb22caa5c550ac66cfc3d1))
* **pitch:** use bounded Q14 products in PitchTracker ([#125](https://github.com/ByteLandTechnology/tunelet/issues/125)) ([9e69547](https://github.com/ByteLandTechnology/tunelet/commit/9e69547531e1ad01f41893541e4735ada6dc3783))
* **pitch:** vectorize cross sums and compute shifts ([eefb67f](https://github.com/ByteLandTechnology/tunelet/commit/eefb67f3ac59de7f83e3450d3c4e6b6df9c27682))
* **resynthesis:** skip silent voice interpolation in harmony ([e56df2b](https://github.com/ByteLandTechnology/tunelet/commit/e56df2b011470354bcfab8eae21ce4fb4fb43a3a))
* **resynthesis:** synthesize one channel when stereo is off ([#102](https://github.com/ByteLandTechnology/tunelet/issues/102)) ([4d52353](https://github.com/ByteLandTechnology/tunelet/commit/4d523534932d0d58f82a4d77a41d587b22bc0f4c))
* **reverb:** use a fixed ring with inline taps ([#86](https://github.com/ByteLandTechnology/tunelet/issues/86)) ([c7b6224](https://github.com/ByteLandTechnology/tunelet/commit/c7b622480227fbf947c82f34e6bca8377c23aa8f))
* shrink formant tables and clear dirty spans on reset ([7490b83](https://github.com/ByteLandTechnology/tunelet/commit/7490b83a23705b53b60310170a1bccef86ebd58f))
* use branchless Q14 shifts and direct biquad sums ([#128](https://github.com/ByteLandTechnology/tunelet/issues/128)) ([1584500](https://github.com/ByteLandTechnology/tunelet/commit/1584500ff633675dc35a3ed3e788f35fd2e45ede))
* **wasm:** decode JS events into a reusable buffer ([#99](https://github.com/ByteLandTechnology/tunelet/issues/99)) ([f19bfe0](https://github.com/ByteLandTechnology/tunelet/commit/f19bfe098c8aefeaa4317fe7ac157e0e9c4bccc1))

## [0.3.4](https://github.com/ByteLandTechnology/tunelet/compare/v0.3.3...v0.3.4) (2026-11-25)


### Bug Fixes

* **midi:** reset harmony on source change and fix correction keys ([#46](https://github.com/ByteLandTechnology/tunelet/issues/46)) ([7dea4b3](https://github.com/ByteLandTechnology/tunelet/commit/7dea4b366f13101571c271969076b86276c807b1))
* **wasm:** sanitize TypeScript declarations and Wasm exports ([#123](https://github.com/ByteLandTechnology/tunelet/issues/123)) ([254ce7d](https://github.com/ByteLandTechnology/tunelet/commit/254ce7d9cbacfaa8eff770c653e3401a847a7847))


### Performance Improvements

* cache Warm reciprocals and accelerate AtanQ14 ([#113](https://github.com/ByteLandTechnology/tunelet/issues/113)) ([7ae0f23](https://github.com/ByteLandTechnology/tunelet/commit/7ae0f2332787399a6ae9870a6d07f210ee525922))
* **formant:** fuse envelope rotations and cache Wasm results ([#30](https://github.com/ByteLandTechnology/tunelet/issues/30)) ([c815dd5](https://github.com/ByteLandTechnology/tunelet/commit/c815dd5dfb6d6aa41dd1df4e7dc7c7b2d7ed324b))
* **resynthesis:** synthesize one channel when stereo is off ([#102](https://github.com/ByteLandTechnology/tunelet/issues/102)) ([4d52353](https://github.com/ByteLandTechnology/tunelet/commit/4d523534932d0d58f82a4d77a41d587b22bc0f4c))
* **reverb:** use a fixed ring with inline taps ([#86](https://github.com/ByteLandTechnology/tunelet/issues/86)) ([c7b6224](https://github.com/ByteLandTechnology/tunelet/commit/c7b622480227fbf947c82f34e6bca8377c23aa8f))

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
