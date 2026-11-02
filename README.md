# tunelet

tunelet is a Rust library for vocal pitch correction, choir, harmony,
formant control and effects. Its input is mono signed 16-bit PCM at 44.1 kHz
or 48 kHz. Its output is stereo or mono PCM. The default Rust build has no
dependencies. For `wasm32-unknown-unknown`, the optional `wasm` feature gives
JavaScript bindings and generated TypeScript declarations.

The processing chain has these stages, in this order:

1. Punch
2. Warm
3. Pitch tracking, correction and voice resynthesis
4. Alien
5. Reverb
6. Output gain

The default configuration enables chromatic pitch correction and uses one
choir voice. It disables the effects and the formant stage.

## Integration

Add the repository as a Git dependency:

```toml
[dependencies]
tunelet = { git = "https://github.com/ByteLandTechnology/tunelet.git" }
```

To prepare a processor, do these steps:

1. Use `SampleRate::try_from(hz)` to check the input rate.
2. Use `PrepareConfig::new(rate, max_block_frames)` to set a nonzero block
   limit.
3. Create a `Processor` with that preparation and a `ProcessorConfig`.

Configuration structures implement `Default`. To select voices and effects,
edit their fields.

| Output layout                  | Function for input      | Function for the tail |
| ------------------------------ | ----------------------- | --------------------- |
| `[[i16; 2]]` stereo frames     | `process`               | `drain`               |
| Separate left and right slices | `process_planar`        | `drain_planar`        |
| Mono slice                     | `process_mono_in_place` | `drain_mono`          |

Each input sample gives one output frame. The channel lengths must be equal,
and each call must fit in the prepared block limit. Short blocks and empty
blocks are permitted. All operations that can fail do their validation before
they change the state or the output.

To apply controls immediately before specific frames, give `TimedEvent`
values. These rules apply to the offsets:

- Offsets start at zero in each block.
- Each offset must be in the block.
- An offset must not be less than the offset before it.

Events with equal offsets execute in slice order. An empty block accepts no
events. Harmony MIDI uses `ControlEvent::Midi`. MIDI for the correction
keyboard uses `ControlEvent::CorrectionMidi`.

## State and timing

Construction allocates the storage of the processor. These operations do not
allocate memory, acquire locks or do I/O:

- Processing
- Configuration
- MIDI
- Feedback
- Reset
- Drain

The crate forbids unsafe Rust.

To get the tail after the last input block, do these steps:

1. Call `begin_drain()` one time.
2. From each drain call, use only the frames that the call returns.

The drain fills the unused output with zeros. The drain has a finite budget,
which can include silence. Completion resets the audio histories and the
feedback. A successful call that processes audio or sets the configuration
cancels the drain. `reset()` also cancels the drain.

`feedback()` reads the pitch information, the stereo peak, the correction
ratio, the frame count and the latency. `take_feedback()` also clears the
peak window. `latency_frames()` gives 1,593 frames of nominal resynthesis
latency at both sample rates. Dry mixes, resampled voices and effects have
different timing.

`reset()` keeps the configuration, the random stream and some controller and
filter state. If you need a fully new state, construct a new processor.

The API documentation gives the parameter ranges, the MIDI behavior and the
reset semantics. To open it, run this command:

```sh
cargo doc --locked --no-deps --open
```

## Build

To build the crate and check the format and the lints, run these commands:

```sh
cargo build --locked --release
cargo fmt --check
cargo clippy --locked --lib -- -D warnings
```

## WebAssembly and TypeScript

The npm package is one ES module for both browsers and Node.js. It contains
these items:

- One Wasm binary.
- The generated bindings.
- A shared `loadTunelet` entry point.

Both environments use the same imports and the same initialization call:

```ts
import { loadTunelet, Tunelet, defaultConfig } from "tunelet";

await loadTunelet();
const config = defaultConfig();
config.resynthesis.pitchRatio = 1.25;
const processor = new Tunelet(48000, 1024, config);

const input = new Int16Array(1024);
const left = new Int16Array(1024);
const right = new Int16Array(1024);
// Fill input with mono PCM before each call; reuse all three arrays.
processor.process(input, left, right);

// After all processing and optional drain calls:
processor.free();
```

Node.js reads the packaged binary from a file URL relative to the module.
Browsers fetch the served Wasm asset. An import of the package does not load
the binary. Call `loadTunelet()` before you use `Tunelet` or `defaultConfig()`.

Concurrent calls share one initialization promise. The package keeps the
result of a successful initialization. If a load fails, the promise rejects,
and you can try again. A Node.js file error goes to the caller.

For a custom asset location, give one of these sources to
`loadTunelet(source)`:

- A URL.
- A fetch Response.
- A byte buffer.
- A compiled `WebAssembly.Module`.
- A promise for one of these.

For a Node.js `file:` source, use a URL object. A string source obeys fetch
rules. The first concurrent call selects the source. Later arguments do not
replace an initialized module. A custom binary must match the generated
bindings.

The package root also exports `loadTunelet` as its default export. For direct
control of Wasm initialization, `tunelet/wasm` gives the generated default
initializer and `initSync`. The package root re-exports `initSync` too. Use
only one initialization entry point for each module instance. The package
gives only ESM exports. CommonJS applications can use `await import("tunelet")`.

Browser bundlers must keep or emit the Wasm asset that
`new URL(..., import.meta.url)` refers to. For a browser deployment without a
bundler, do these steps:

1. Serve `dist/` and `pkg/`, and keep their relative layout.
2. Map the `tunelet` import to `dist/index.js`, or import that URL directly.

Serve the assets over HTTP(S). The package does not support a local `file:`
page that opens the JavaScript.

### Build the shared package

Install Node.js and `wasm-pack`. Then run these commands to add the
`wasm32-unknown-unknown` Rust target, install the development dependencies
and make the archive:

```sh
rustup target add wasm32-unknown-unknown
npm ci
npm pack
```

`npm pack` builds the Wasm bindings and the TypeScript entry point. Then it
creates `tunelet-<version>.tgz`. To use this archive in a browser project or
a Node.js project, install it with this command:

```sh
npm install /path/to/tunelet-<version>.tgz
```

For local use, it is not necessary to publish the package to npm. To build
without an archive, run `npm run build`.

The build writes one `web`-target binding into `pkg/` and the shared entry
into `dist/`. Git ignores both directories. The archive contains only these
items:

- The runtime entry.
- The declarations.
- The bindings.
- The binary.
- The package metadata.
- The README.

The `build:wasm` script uses `--no-opt`. This flag disables the separate
Binaryen pass, but Rust still builds in release mode. The TypeScript and
Node.js type declarations are only development dependencies. Native Rust
builds use Cargo and do not need Node.js or npm.

| Operation                         | TypeScript API                                           |
| --------------------------------- | -------------------------------------------------------- |
| Shared initialization             | `loadTunelet(source?)`                                   |
| Mono PCM to planar stereo         | `process(input, left, right, events?)`                   |
| In-place mono downmix             | `processMono(audio, events?)`                            |
| Configuration                     | `config()`, `setConfig(config)`, `defaultConfig()`       |
| Information about the preparation | `sampleRate`, `maxBlockFrames`, `latencyFrames`          |
| Feedback                          | `feedback()`, `takeFeedback()`                           |
| Finite tail                       | `beginDrain()`, `drain(left, right)`, `drainMono(audio)` |
| Configured tail budget            | `tailFrames()`                                           |
| Reset or release memory           | `reset()`, `free()`                                      |

All audio buffers are `Int16Array` buffers, and they must fit in the prepared
frame limit. Stereo output arrays must not overlap. The input can overlap an
output, because the binding copies the input first. Buffers must stay attached
during each call. Web Audio uses Float32 samples, which you must convert to
and from 16-bit PCM. This package does not install an AudioWorklet or an
integration with an audio device.

Configuration objects contain all fields. They use camelCase names and show
enums as string literals. To change the configuration, do these steps:

1. Start from `defaultConfig()` or `config()`.
2. Edit the necessary fields.
3. Call `setConfig()`.

If a field is missing or a value is not valid, the call throws a JavaScript
`Error`. The conversion ignores extra object properties. The generated
declarations contain the parameter documentation from the Rust types.
`resynthesis.randomSeed` and the `processedFrames` field of feedback use
`bigint`. Frame offsets, block lengths and latency use `number`.

In feedback, `targetMidiNote` is `undefined` in these cases, which are also
the cases where `voiced` is `false`:

- The kept pitch estimate is unvoiced. This includes the time before the
  first detection and after a reset or a drain completion.
- The estimated frequency is outside the inclusive reporting bounds
  `resynthesis.pitch.minFrequencyHz` and `resynthesis.pitch.maxFrequencyHz`.

The optional event argument is a `TimedEvent[]`. Controls use a `type`
discriminant and a `value` payload. The reset control has no payload. For
example, `{ offset: 0, event: { type: "outputGain", value: 0.5 } }` changes
the final gain before the first frame.

A harmony Note On event uses
`{ offset: 0, event: { type: "midi", value: { type: "noteOn", channel: 0, note: 60, velocity: 100 } } }`.
Use `correctionMidi` for the separate correction keyboard. The native rules
for event order and for the validation of the whole block also apply to these
bindings.

The constructor allocates reusable PCM buffers. A successful call that
processes audio without events does not allocate new Rust PCM storage.
However, the call copies the audio across the boundary between JavaScript and
Wasm. These items can allocate:

- Object conversion.
- Supplied events.
- Errors.
- Feedback.

The generated glue can also make temporary views. This binding is for general
use, and it does not guarantee hard real-time behavior for allocation or
scheduling. Drain completion and reset keep the same state as the equivalent
Rust functions.

## License

tunelet is available under the MIT License. Refer to [LICENSE](LICENSE).
