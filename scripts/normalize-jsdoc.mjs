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
  ["[`Processor::feedback`]", "`Tunelet.feedback`"],
  ["[`Processor::take_feedback`]", "`Tunelet.takeFeedback`"],
  ["[`Processor::telemetry`]", "`Tunelet.telemetry`"],
  ["[`Processor::take_telemetry`]", "`Tunelet.takeTelemetry`"],
  ["[`Processor::latency_frames`]", "`Tunelet.latencyFrames`"],
  ["[`Processor::formant_envelope`]", "`Tunelet.formantEnvelope`"],
  ["[`Feedback::processed_frames`]", "`Feedback.processedFrames`"],
  ["[`Feedback::voiced`]", "`Feedback.voiced`"],
  [
    "[`crate::ResynthesisConfig::choir_voices`]",
    "`ResynthesisConfig.choirVoices`",
  ],
  ["[`crate::AlienConfig::loop_frames`]", "`AlienConfig.loopFrames`"],
  ["[`crate::PitchConfig`]", "`PitchConfig`"],
  ["[`TimedEvent`]", "`TimedEvent`"],
  ["[`HarmonyScale`]", "`HarmonyScale`"],
  ["[`Feedback`]", "`Feedback`"],
  ["[`HarmonyConfig`]", "`HarmonyConfig`"],
  ["[`Self::I`]", '`"i"`'],
  ["[`Self::Echo`]", '`"echo"`'],
  ["[`ReverbMode::Echo`]", '`"echo"`'],
  ["[`Self::MidiChannels`]", '`"midiChannels"`'],
  ["[`HarmonySource::MidiChannels`]", '`"midiChannels"`'],
  ["[`HarmonySource::FixedInterval`]", '`"fixedInterval"`'],
  ["[`HarmonySource::SmartInterval`]", '`"smartInterval"`'],
  ["[`HarmonySource::ChordByName`]", '`"chordByName"`'],
  ["[`HarmonySource::ChordByDegree`]", '`"chordByDegree"`'],
  ["[`MultiVoiceMode::Choir`]", '`"choir"`'],
  ["`None`", "`undefined`"],
  ["`u64::MAX`", "`2n ** 64n - 1n`"],
  ["`0x1234_abcd`", "`0x1234abcdn`"],
  ["`max_frequency_hz`", "`maxFrequencyHz`"],
  ["`min_frequency_hz`", "`minFrequencyHz`"],
  ["`stereo_output`", "`stereoOutput`"],
  ["`retune_ms`", "`retuneMs`"],
  ["`attack_ms`", "`attackMs`"],
];

/** Multi-line prose replacements for Rust-only traits, methods and borrows. */
const PROSE_REPLACEMENTS = [
  [
    /The call borrows\s+the slice only for that call\. There is no heap allocation and no deferred\s+queue\./g,
    "The processor\napplies controls during that call with no deferred queue.",
  ],
  [/\bin the event slice\b/g, "in the event array"],
  [/\bexecute in slice order\b/g, "execute in array order"],
  [/\bearlier in the slice\b/g, "earlier in the array"],
  [
    /A scale for \[`NoteMask::from_key_scale`\]\./g,
    "A major, natural minor or chromatic scale name.",
  ],
  [
    /\s+`Default` gives a snapshot\s+with all zeros and no target\. It does not read the nominal latency of a\s+prepared processor\./g,
    "",
  ],
  [/\s+`Default` gives a snapshot with all zeros\./g, ""],
  [
    /The\s+default is \[`PitchConfig::default`\]\./g,
    "Refer to `PitchConfig` for the field defaults.",
  ],
  [
    /The default is \[`ResynthesisConfig::default`\]\./g,
    "Refer to `ResynthesisConfig` for the field defaults.",
  ],
  [
    /The default is\s+\[`NoteMask::CHROMATIC`\]\./g,
    "The default enables all twelve pitch classes (`true` in every slot).",
  ],
  [
    /The default is \[`Self::CHROMATIC`\]\. All combinations of flags are valid\.\s+This includes \[`Self::NONE`\]\./g,
    "The default enables all twelve pitch classes (`true` in every slot). All\ncombinations of flags are valid, which includes all `false` values.",
  ],
  [
    /Use \[`crate::ControlEvent::Midi`\] for harmony\. Use\s+\[`crate::ControlEvent::CorrectionMidi`\] for the independent correction\s+keyboard\. The typed representation can contain `u8` values that are not\s+valid\. \[`Self::validate`\] and the event preflight of the processor check\s+the MIDI ranges\./g,
    'Use `{ type: "midi", value }` in `ControlEvent` for harmony. Use\n`{ type: "correctionMidi", value }` for the independent correction\nkeyboard. Number fields can hold values outside the MIDI ranges, and\n`Tunelet` checks the ranges before it processes a block.',
  ],
  [
    /Change a copy, then apply it with \[`crate::Processor::set_config`\] or\s+\[`crate::ControlEvent::Configure`\]\./g,
    'Change a copy, then apply it with `Tunelet.setConfig` or\n`{ type: "configure", value }` in `ControlEvent`.',
  ],
  [
    /The MIDI sources use\s+\[`crate::ControlEvent::Midi`\]\./g,
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
    / \(inclusive\),\s*(-?\d+(?:\.\d+)?)\.\.=(-?\d+(?:[./]\d+)?%?)/g,
    ", from $1 to $2 (inclusive)",
  );
  text = text.replace(
    /\bin\s+(-?\d+(?:\.\d+)?)\.\.=(-?\d+(?:[./]\d+)?%?)/g,
    "from $1 to $2 (inclusive)",
  );
  text = text.replace(
    /(-?\d+(?:\.\d+)?)\.\.=(-?\d+(?:[./]\d+)?%?)/g,
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

/** Normalize all JSDoc blocks in one generated file. */
function normalizeFile(filePath) {
  const original = fs.readFileSync(filePath, "utf8");
  const updated = original.replace(/\/\*\*[\s\S]*?\*\//g, (block) =>
    normalizeBlock(block, filePath),
  );
  fs.writeFileSync(filePath, updated, "utf8");
}

fs.rmSync(path.join(pkgDir, ".gitignore"), { force: true });
normalizeFile(path.join(pkgDir, "tunelet.d.ts"));
normalizeFile(path.join(pkgDir, "tunelet.js"));
