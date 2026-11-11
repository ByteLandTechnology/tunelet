# AGENTS.md

tunelet is a Rust library for vocal pitch correction, choir, harmony,
formant control and effects. An optional `wasm` feature and an npm package
make the library available to browsers and Node.js. The output is
sample-exact fixed-point audio. Make small changes. If the task
does not tell you to change the behavior, keep the current
behavior. Do not change the style of code that you do not change
for other reasons.

## Read first

- `README.md`: the integration, the timing, the drain and the npm package.
- `src/lib.rs`: the crate contract. This contract includes the signal
  chain, the events, the lifecycle and the execution model.
- The module that you will change and its public configuration type.

## Writing standard

Write all documentation, comments and commit messages in ASD-STE100
Simplified Technical English (STE). This rule applies to `README.md`,
this file, rustdoc, code comments, TypeScript comments and commit messages.

- Use simple words with one meaning. Use the same word for the same
  thing every time.
- Use technical names (types, functions, crates, tools, units) as they are.
- Do not use more than three nouns in a row.
- Keep the articles (`the`, `a`, `an`). Do not use telegraphic style.
- Use the simple present, simple past or simple future tense.
- Use the active voice where possible.
- Do not use `-ing` forms or phrasal verbs.
- Use `can` and `must`. Do not use `should`, `may`, `might`, `would`
  or `could`.
- Write instructions in the imperative, with no more than 20 words.
- Write descriptive sentences with no more than 25 words.
- Put one topic in each sentence. Put a condition first: "If X, do Y."
- Write paragraphs with one topic and no more than six sentences.

## Core rules

- Make the smallest possible change. Do not rename, move or format
  code that is not part of the task.
- The public API is all items that `src/lib.rs` exports, `src/wasm.rs`
  and `src/index.ts`. Do not add, change or remove public items if the
  task does not tell you to. Keep the Rust, Wasm and TypeScript APIs
  equivalent.
- Keep the crate lints in `src/lib.rs`: `#![forbid(unsafe_code)]`,
  `#![deny(missing_docs)]`, `#![deny(rustdoc::broken_intra_doc_links)]`
  and `#![warn(unreachable_pub)]`. Each public item must have rustdoc
  that includes its parameter ranges.
- In the Rust API, only construction can allocate memory. Audio calls,
  controls, feedback, reset and drain must not allocate memory, acquire
  locks or do I/O. The Wasm binding can also allocate when it converts
  objects, events, feedback and errors.
- The fixed-point arithmetic uses explicit wrapping and quantization. If
  you change the operation order, the rounding or a table value, the
  output samples change. Treat this as a change of behavior.
- A call that can fail must validate the full block or configuration
  before it changes the state or the output. Keep this rule.
- The default build has no dependencies. Make each Wasm dependency
  optional, enable it from the `wasm` feature and put it in the
  `cfg(target_arch = "wasm32")` dependency table. For the binding
  derives of shared types, use
  `cfg_attr(all(feature = "wasm", target_arch = "wasm32"), ...)`.
- Do not change user changes, untracked files or ignored files that
  are not part of the task. The ignored files include `target/`,
  `pkg/`, `dist/`, `node_modules/` and `*.tgz`.

## Repository map

- `src/lib.rs`: the crate documentation, the lints and the exports.
- `src/processor.rs`, `src/config.rs`, `src/events.rs`, `src/error.rs`:
  `Processor`, the configuration, the timed events and the errors.
- `src/pitch.rs`, `src/pitch/`, `src/scale.rs`, `src/midi.rs`: the
  pitch tracker, the correction scales and MIDI.
- `src/lpc.rs`, `src/formant.rs`: the LPC analysis and the formant
  control.
- `src/filter.rs`: the Q14 biquad filters.
- `src/resynthesis.rs`, `src/resynthesis/`: the resynthesis and the
  audio histories.
- `src/harmony.rs`, `src/harmony/`: the harmony voices and the chords.
- `src/effects.rs`, `src/effects/`: the Punch, Warm, Alien and reverb
  effects.
- `src/numeric.rs`, `src/numeric/`: the fixed-point functions, the
  random number generator and the lookup tables.
- `src/wasm.rs`: the wasm-bindgen and tsify bindings (`wasm` feature).
- `src/index.ts`: the `loadTunelet` entry point. See also `package.json`
  and `tsconfig.json`.

## Build and verify

```sh
cargo build --locked --release
cargo fmt --check
cargo clippy --locked --lib -- -D warnings
cargo doc --locked --no-deps
```

If you change the Wasm or TypeScript code, also run these commands.
`npm run build` compiles `src/wasm.rs` and `src/index.ts`:

```sh
rustup target add wasm32-unknown-unknown
npm ci
npm run build
```

- Make sure that there are no new warnings. This includes broken
  rustdoc links.
- The crate has no tests (`test = false`). Do not add tests or test
  support if the task does not tell you to.
- To verify a DSP change, render PCM from a fixed input and seed before
  and after the change. Then compare the two outputs.

## Guidance for specific changes

- Configuration fields: in the same change, update the `validate`
  method of the configuration type, the rustdoc ranges and `README.md`.
  The generated TypeScript declarations come from these Rust types and
  their rustdoc.
- Signal chain or latency: update `LATENCY_FRAMES` in
  `src/resynthesis.rs`, the tail budget in `tail_frames`, the crate
  documentation and `README.md`.
- Lock files: keep `Cargo.lock` and `package-lock.json` in the
  repository. Build with `--locked` and `npm ci`.
- Ignore rules: do not edit the template part of `.gitignore`. To
  update it, run the `npx gitignore` commands that the file lists, in
  the same order. Then add the "Project rules" section again.
- Releases: release-please sets the versions and writes `CHANGELOG.md`.
  Do not change the version in `Cargo.toml`, `Cargo.lock`,
  `package.json`, `package-lock.json` or `.release-please-manifest.json`
  by hand. The commit types and the `!` mark set the next version. The
  release workflow is `.github/workflows/release.yml`. It publishes to
  npm and crates.io through trusted publishing, so do not rename it.

## Prohibited changes

- Do not format large areas of code. Do not simplify numeric code only
  to make it look better.
- Do not change fixed-point calculations to floating-point calculations.
- Do not edit the generated files in `pkg/` or `dist/`.

## Work method

- Reproduce a problem before you fix it. Find the root cause first.
- If the necessary audio behavior is not clear, ask the user.
- Complex DSP code usually keeps the output exact. Read the Git history
  before you make it simpler.

## Commit guidance

- Before you commit a code change, run the commands in "Build and verify".
- Put one topic in each commit.
- Do not push. The user pushes the commits.
- Use Conventional Commits (`type(scope): summary`). Use a scope that
  names the changed area (for example, `pitch`, `reverb`, `wasm` or
  `npm`), or use no scope. Add `!` for a breaking change.
- Write the body as a short `-` list of the changes.
- Start each line with an imperative verb and write one action in each
  item. Follow the "Writing standard" section.
- Keep each line at or below 72 columns. If a line is too long, make
  the text shorter. Do not continue it on a second line.
- Write a message with more than one line in a file. Do not use escaped
  `\n` in a shell `-m` argument.
- Do not add agent attribution, for example `Co-Authored-By`,
  `Assisted-by` or session links.
