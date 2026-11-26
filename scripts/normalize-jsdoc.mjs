/**
 * This script cleans `pkg/.gitignore` and normalizes Rustdoc comments in
 * `pkg/tunelet.d.ts` and `pkg/tunelet.js` into idiomatic TypeScript JSDoc.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const pkgDir = path.join(repoRoot, "pkg");

/** Exact Rust intra-doc link and literal replacements for TypeScript JSDoc. */
const EXACT_REPLACEMENTS = [
  ["[`voiced`](Self::voiced)", "`voiced`"],
  ["[`voice_ratios`](Self::voice_ratios)", "`voiceRatios`"],
  ["[`voice_gains`](Self::voice_gains)", "`voiceGains`"],
  ["[`voice_count`](Self::voice_count)", "`voiceCount`"],
  ["[`alien_loop_frames`](Self::alien_loop_frames)", "`alienLoopFrames`"],
  ["[`crate::Processor::set_config`]", "`Tunelet.setConfig`"],
  ["[`crate::Processor::control_activity`]", "`Tunelet.controlActivity`"],
  ["[`Processor::control_activity`]", "`Tunelet.controlActivity`"],
  ["[`ProcessorConfig::control_activity`]", "`Tunelet.controlActivity`"],
  ["[`Processor::feedback`]", "`Tunelet.feedback`"],
  ["[`Processor::take_feedback`]", "`Tunelet.takeFeedback`"],
  ["[`Processor::telemetry`]", "`Tunelet.telemetry`"],
  ["[`Processor::take_telemetry`]", "`Tunelet.takeTelemetry`"],
  ["[`Processor::latency_frames`]", "`Tunelet.latencyFrames`"],
  ["[`Processor::formant_envelope`]", "`Tunelet.formantEnvelope`"],
  ["[`Processor::process_planar_f32`]", "the native `f32` stereo method"],
  ["[`Processor::process_mono_in_place_f32`]", "the native `f32` mono method"],
  ["[`Processor`]", "the native processor"],
  ["[`Feedback::processed_frames`]", "`Feedback.processedFrames`"],
  ["[`Feedback::voiced`]", "`Feedback.voiced`"],
  [
    "[`crate::ResynthesisConfig::choir_voices`]",
    "`ResynthesisConfig.choirVoices`",
  ],
  ["[`ResynthesisConfig::choir_voices`]", "`ResynthesisConfig.choirVoices`"],
  ["[`ResynthesisConfig::choir_detune`]", "`ResynthesisConfig.choirDetune`"],
  ["[`ResynthesisConfig::choir_delay`]", "`ResynthesisConfig.choirDelay`"],
  [
    "[`ResynthesisConfig::choir_spread_percent`]",
    "`ResynthesisConfig.choirSpreadPercent`",
  ],
  ["[`ResynthesisConfig::harmony_gain`]", "`ResynthesisConfig.harmonyGain`"],
  ["[`crate::HarmonyConfig::source`]", "`HarmonyConfig.source`"],
  ["[`crate::HarmonyConfig::key`]", "`HarmonyConfig.key`"],
  ["[`crate::HarmonyConfig::scale`]", "`HarmonyConfig.scale`"],
  [
    "[`crate::HarmonyConfig::chromatic_intervals`]",
    "`HarmonyConfig.chromaticIntervals`",
  ],
  [
    "[`crate::HarmonyConfig::diatonic_intervals`]",
    "`HarmonyConfig.diatonicIntervals`",
  ],
  ["[`crate::HarmonyConfig::chord`]", "`HarmonyConfig.chord`"],
  ["[`crate::HarmonyConfig::degree`]", "`HarmonyConfig.degree`"],
  ["[`crate::HarmonyConfig::inversion`]", "`HarmonyConfig.inversion`"],
  [
    "[`crate::HarmonyConfig::register_percent`]",
    "`HarmonyConfig.registerPercent`",
  ],
  [
    "[`crate::HarmonyConfig::voicing_spread_percent`]",
    "`HarmonyConfig.voicingSpreadPercent`",
  ],
  ["[`crate::HarmonyConfig::portamento_ms`]", "`HarmonyConfig.portamentoMs`"],
  ["[`crate::HarmonyConfig::voices`]", "`HarmonyConfig.voices`"],
  [
    "[`crate::HarmonyConfig::midi_channel_map`]",
    "`HarmonyConfig.midiChannelMap`",
  ],
  ["[`HarmonyConfig::midi_channel_map`]", "`HarmonyConfig.midiChannelMap`"],
  [
    "[`crate::HarmonyConfig::midi_base_channel`]",
    "`HarmonyConfig.midiBaseChannel`",
  ],
  ["[`HarmonyConfig::midi_base_channel`]", "`HarmonyConfig.midiBaseChannel`"],
  ["[`crate::CorrectionConfig::notes`]", "`CorrectionConfig.notes`"],
  ["[`CorrectionConfig::notes`]", "`CorrectionConfig.notes`"],
  [
    "[`crate::ControlEvent::CorrectionMidi`]",
    '`{ type: "correctionMidi", value }`',
  ],
  ["[`MidiEvent::AllNotesOff`]", '`{ type: "allNotesOff" }`'],
  ["[`ReverbConfig::mode`]", "`ReverbConfig.mode`"],
  ["[`ReverbConfig::mix`]", "`ReverbConfig.mix`"],
  ["[`ReverbConfig::diffusion`]", "`ReverbConfig.diffusion`"],
  ["[`ReverbConfig::decay`]", "`ReverbConfig.decay`"],
  ["[`ReverbConfig::brightness_hz`]", "`ReverbConfig.brightnessHz`"],
  ["[`crate::AlienConfig::loop_frames`]", "`AlienConfig.loopFrames`"],
  ["[`crate::PitchConfig`]", "`PitchConfig`"],
  ["[`ProcessorConfig`]", "`ProcessorConfig`"],
  ["[`ControlActivity`]", "`ControlActivity`"],
  ["[`TimedEvent`]", "`TimedEvent`"],
  ["[`HarmonyScale`]", "`HarmonyScale`"],
  ["[`Feedback`]", "`Feedback`"],
  ["[`HarmonyConfig`]", "`HarmonyConfig`"],
  ["[`MidiChannelMap`]", "`MidiChannelMap`"],
  ["[`CorrectionMidiMode`]", "`CorrectionMidiMode`"],
  ["[`Self::I`]", '`"i"`'],
  ["[`Self::Echo`]", '`"echo"`'],
  ["[`ReverbMode::Echo`]", '`"echo"`'],
  ["[`Self::MidiOmni`]", '`"midiOmni"`'],
  ["[`HarmonySource::MidiOmni`]", '`"midiOmni"`'],
  ["[`Self::MidiChannels`]", '`"midiChannels"`'],
  ["[`HarmonySource::MidiChannels`]", '`"midiChannels"`'],
  ["[`HarmonySource::FixedInterval`]", '`"fixedInterval"`'],
  ["[`HarmonySource::SmartInterval`]", '`"smartInterval"`'],
  ["[`HarmonySource::ChordByName`]", '`"chordByName"`'],
  ["[`HarmonySource::ChordByDegree`]", '`"chordByDegree"`'],
  ["[`MidiChannelMap::BaseWindow`]", '`"baseWindow"`'],
  ["[`MidiChannelMap::WrapFour`]", '`"wrapFour"`'],
  ["[`crate::CorrectionMidiMode::Momentary`]", '`"momentary"`'],
  ["[`CorrectionMidiMode::Momentary`]", '`"momentary"`'],
  ["[`crate::CorrectionMidiMode::Latch`]", '`"latch"`'],
  ["[`CorrectionMidiMode::Latch`]", '`"latch"`'],
  ["[`MultiVoiceMode::Choir`]", '`"choir"`'],
  ["`None`", "`undefined`"],
  ["`u64::MAX`", "`2n ** 64n - 1n`"],
  ["`0x1234_abcd`", "`0x1234abcdn`"],
  ["`max_frequency_hz`", "`maxFrequencyHz`"],
  ["`min_frequency_hz`", "`minFrequencyHz`"],
  ["`stereo_output`", "`stereoOutput`"],
  ["`retune_ms`", "`retuneMs`"],
  ["`pitch_ratio`", "`pitchRatio`"],
  ["`flex_tune`", "`flexTune`"],
  ["`midi_channel_map`", "`midiChannelMap`"],
  ["`midi_base_channel..=midi_base_channel + 3`", "`midiBaseChannel..=midiBaseChannel + 3`"],
  ["`c - midi_base_channel`", "`c - midiBaseChannel`"],
  ["`midi_base_channel + n`", "`midiBaseChannel + n`"],
  ["`midi_base_channel`", "`midiBaseChannel`"],
  ["`midi_mode`", "`midiMode`"],
  ["`attack_ms`", "`attackMs`"],
  ["`gain_db`", "`gainDb`"],
  ["`ceiling_db`", "`ceilingDb`"],
];

/** Multi-line prose replacements for Rust-only traits, methods and borrows. */
const PROSE_REPLACEMENTS = [
  [
    /The\s+call\s+borrows\s+the\s+slice\s+only\s+for\s+that\s+call\.\s+There\s+is\s+no\s+heap\s+allocation\s+and\s+no\s+deferred\s+queue\./g,
    "The processor\napplies controls during that call with no deferred queue.",
  ],
  [/\bin\s+the\s+event\s+slice\b/g, "in the event array"],
  [/\bexecute\s+in\s+slice\s+order\b/g, "execute in array order"],
  [/\bearlier\s+in\s+the\s+slice\b/g, "earlier in the array"],
  [
    /A\s+scale\s+for\s+\[`NoteMask::from_key_scale`\]\./g,
    "A major, natural minor or chromatic scale name.",
  ],
  [
    /Get\s+this\s+snapshot\s+from\s+\[`ProcessorConfig::control_activity`\]\s+or\s+\[`crate::Processor::control_activity`\]\./g,
    "Get this snapshot from `Tunelet.controlActivity`.",
  ],
  [/\s+`Default`\s+sets\s+all\s+flags\s+to\s+`false`\./g, ""],
  [
    /\s+`Default`\s+gives\s+a\s+snapshot\s+with\s+all\s+zeros\s+and\s+no\s+target\.\s+It\s+does\s+not\s+read\s+the\s+nominal\s+latency\s+of\s+a\s+prepared\s+processor\./g,
    "",
  ],
  [/\s+`Default`\s+gives\s+a\s+snapshot\s+with\s+all\s+zeros\./g, ""],
  [
    /The\s+default\s+is\s+\[`PitchConfig::default`\]\./g,
    "Refer to `PitchConfig` for the field defaults.",
  ],
  [
    /The\s+default\s+is\s+\[`ResynthesisConfig::default`\]\./g,
    "Refer to `ResynthesisConfig` for the field defaults.",
  ],
  [
    /The\s+default\s+is\s+\[`NoteMask::CHROMATIC`\]\./g,
    "The default enables all twelve pitch classes (`true` in every slot).",
  ],
  [
    /The\s+default\s+is\s+\[`Self::CHROMATIC`\]\.\s+All\s+combinations\s+of\s+flags\s+are\s+valid\.\s+This\s+includes\s+\[`Self::NONE`\]\./g,
    "The default enables all twelve pitch classes (`true` in every slot). All\ncombinations of flags are valid, which includes all `false` values.",
  ],
  [
    /Use\s+\[`crate::ControlEvent::Midi`\]\s+for\s+harmony\.\s+Use\s+\[`crate::ControlEvent::CorrectionMidi`\]\s+for\s+the\s+independent\s+correction\s+keyboard\.\s+The\s+typed\s+representation\s+can\s+contain\s+`u8`\s+values\s+that\s+are\s+not\s+valid\.\s+\[`Self::validate`\]\s+and\s+the\s+event\s+preflight\s+of\s+the\s+processor\s+check\s+the\s+MIDI\s+ranges\./g,
    'Use `{ type: "midi", value }` in `ControlEvent` for harmony. Use\n`{ type: "correctionMidi", value }` for the independent correction\nkeyboard. Number fields can hold values outside the MIDI ranges, and\n`Tunelet` checks the ranges before it processes a block.',
  ],
  [
    /Change\s+a\s+copy,\s+then\s+apply\s+it\s+with\s+\[`crate::Processor::set_config`\]\s+or\s+\[`crate::ControlEvent::Configure`\]\./g,
    'Change a copy, then apply it with `Tunelet.setConfig` or\n`{ type: "configure", value }` in `ControlEvent`.',
  ],
  [
    /The\s+MIDI\s+sources\s+use\s+\[`crate::ControlEvent::Midi`\]\./g,
    'The MIDI sources use\n`{ type: "midi", value }` in `ControlEvent`.',
  ],
];

/** Convert clean Markdown text from a JSDoc block into TypeScript JSDoc text. */
function transformCommentText(input) {
  let text = input.replace(/(?:^|\n)# Errors\n\n/g, (match) =>
    match.startsWith("\n") ? "\n@throws " : "@throws ",
  );

  for (const [pattern, replacement] of PROSE_REPLACEMENTS) {
    text = text.replace(pattern, replacement);
  }

  for (const [source, target] of EXACT_REPLACEMENTS) {
    text = text.replaceAll(source, target);
  }

  text = text.replace(
    /\bin\s+`([^`\n]+?)\.\.=([^`\n]+?)`/g,
    "from `$1` to `$2` (inclusive)",
  );
  text = text.replace(
    /`([^`\n]+?)\.\.=([^`\n]+?)`/g,
    "`$1` to `$2` (inclusive)",
  );

  const bound =
    "(?:-?\\d+(?:\\.\\d+)?(?:\\s*\\/\\s*\\d+)?%?|[A-Za-z_][A-Za-z0-9_]*)";
  text = text.replace(
    new RegExp(` \\(inclusive\\),\\s*(${bound})\\.\\.=(${bound})`, "g"),
    ", from $1 to $2 (inclusive)",
  );
  text = text.replace(
    new RegExp(`\\bin\\s+(${bound})\\.\\.=(${bound})`, "g"),
    "from $1 to $2 (inclusive)",
  );
  text = text.replace(
    new RegExp(`(${bound})\\.\\.=(${bound})`, "g"),
    "$1 to $2 (inclusive)",
  );

  return text;
}

/** Check that a normalized JSDoc text has no leftover Rustdoc constructs. */
function assertCleanCommentText(text, filePath) {
  assert.doesNotMatch(
    text,
    /(?:^|\n)#+\s/,
    `Unexpected Markdown heading in ${filePath}:\n${text}`,
  );
  assert.ok(
    !text.includes("::"),
    `Unexpected Rust path separator in ${filePath}:\n${text}`,
  );
  assert.ok(
    !text.includes("..="),
    `Unexpected Rust inclusive range in ${filePath}:\n${text}`,
  );
  assert.doesNotMatch(
    text,
    /\b(?:None|NONE)\b/,
    `Unexpected Rust None literal in ${filePath}:\n${text}`,
  );
  assert.doesNotMatch(
    text,
    /\[`[^`]+`\]/,
    `Unexpected Rust intra-doc link in ${filePath}:\n${text}`,
  );
}

/** Normalize one `/** ... *\/` JSDoc block and preserve its indentation. */
function normalizeBlock(block, filePath) {
  if (!block.includes("\n")) {
    const inner = block.slice(3, -2).trim();
    const normalized = transformCommentText(inner);
    assertCleanCommentText(normalized, filePath);
    return `/** ${normalized} */`;
  }

  const lines = block.split("\n");
  const lastLine = lines[lines.length - 1];
  const indentMatch = /^([ \t]*)\*\/$/.exec(lastLine);
  assert.ok(indentMatch, `Malformed JSDoc end in ${filePath}: ${lastLine}`);
  const starPrefix = indentMatch[1];

  const innerLines = lines
    .slice(1, -1)
    .map((line) => line.replace(/^[ \t]*\*(?: |$)/, ""));
  const normalized = transformCommentText(innerLines.join("\n"));
  assertCleanCommentText(normalized, filePath);

  const rebuiltLines = normalized
    .split("\n")
    .map((line) => (line.length === 0 ? `${starPrefix}*` : `${starPrefix}* ${line}`));

  return [`/**`, ...rebuiltLines, `${starPrefix}*/`].join("\n");
}

/** Normalize all JSDoc blocks in a generated source text. */
export function normalizeJsDoc(rawSource, filePath = "<memory>") {
  const newline = rawSource.includes("\r\n") ? "\r\n" : "\n";
  const lfSource = rawSource.replace(/\r\n/g, "\n");
  const updated = lfSource.replace(/\/\*\*[\s\S]*?\*\//g, (block) =>
    normalizeBlock(block, filePath),
  );
  return newline === "\r\n" ? updated.replace(/\n/g, "\r\n") : updated;
}

/** Normalize all JSDoc blocks in one generated file. */
function normalizeFile(filePath) {
  const original = fs.readFileSync(filePath, "utf8");
  const updated = normalizeJsDoc(original, filePath);
  fs.writeFileSync(filePath, updated, "utf8");
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  fs.rmSync(path.join(pkgDir, ".gitignore"), { force: true });
  normalizeFile(path.join(pkgDir, "tunelet.d.ts"));
  normalizeFile(path.join(pkgDir, "tunelet.js"));
}
