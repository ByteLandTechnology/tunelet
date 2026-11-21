# Changelog

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
