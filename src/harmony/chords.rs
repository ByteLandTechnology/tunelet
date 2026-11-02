//! Musical lookup tables and the placement of chords in four voices or fewer.
//!
//! The table rows use the discriminant order of the related public enums.
//! The chord placement applies the inversion and the key to named chords and
//! degree chords. Then it selects a register and searches octave offsets for
//! the voicing spread. Each tone stays with its gain when the tones are
//! sorted. If two candidates have the same score, the traversal order selects
//! the result.

use super::{HarmonyConfig, HarmonySource, HARMONY_VOICES};
use crate::numeric::fixed::{Q12, Q6};

/// Seven semitone offsets of the degrees for each `HarmonyScale`.
/// `MajorWithoutSeventh` repeats its sixth in the last slot to keep seven
/// degrees.
pub(super) const SCALE_TONES: [[i32; 7]; 7] = [
    [0, 2, 4, 5, 7, 9, 11],
    [0, 2, 4, 5, 7, 9, 9],
    [0, 2, 4, 5, 7, 9, 10],
    [0, 2, 3, 5, 7, 8, 10],
    [0, 2, 3, 5, 7, 8, 11],
    [0, 2, 3, 5, 7, 9, 10],
    [0, 2, 3, 5, 7, 9, 11],
];
/// Semitone offsets for each `ChordName` in discriminant order. Each row has
/// three or four tones.
const NAMED: [&[i32]; 17] = [
    &[0, 4, 7],
    &[0, 4, 7, 9],
    &[0, 4, 7, 11],
    &[0, 5, 7, 11],
    &[0, 3, 7],
    &[0, 3, 7, 9],
    &[0, 3, 7, 10],
    &[0, 3, 6, 10],
    &[0, 3, 6],
    &[0, 3, 6, 9],
    &[0, 4, 7, 10],
    &[0, 4, 6, 10],
    &[0, 4, 8],
    &[0, 4, 8, 10],
    &[0, 5, 7],
    &[0, 2, 7],
    &[0, 5, 7, 10],
];
/// Zero-based diatonic offsets for each `ChordDegree` in discriminant order.
///
/// Values above six continue into the next octave.
const DEGREE: [&[i32]; 13] = [
    &[0, 2, 4],
    &[1, 3, 5],
    &[1, 3, 5, 7],
    &[2, 4, 6],
    &[2, 4, 6, 8],
    &[3, 5, 7],
    &[3, 5, 7, 9],
    &[4, 6, 8],
    &[4, 6, 8, 10],
    &[5, 7, 9],
    &[5, 7, 9, 11],
    &[6, 8, 10],
    &[6, 8, 10, 12],
];

/// Returns the chord tones in ascending order, their Q12 gains and a count
/// of 0..=4.
///
/// The count gives the number of used entries at the start of the arrays.
/// The tones are semitone offsets from C4, the reference of the controller.
/// The controller converts them to periods later. Nonzero MIDI periods
/// identify the active input slots. Only MIDI chords use the returned
/// velocity gains. The other sources keep sentinel gains, which are not used.
///
/// All controls must be validated before the call. The count is zero for a
/// source that is not a chord source. It is also zero for a MIDI chord
/// without active notes. Named chords and degree chords
/// apply the inversion and the key. All chord sources apply the register and
/// the spread. The function examines at most 64 octave combinations. Only a
/// higher score replaces the best candidate, so the first candidate wins a
/// tie.
pub(super) fn voice_chord(
    config: &HarmonyConfig,
    midi_note: [i32; HARMONY_VOICES],
    midi_period: [Q6; HARMONY_VOICES],
    midi_gain: [Q12; HARMONY_VOICES],
) -> ([i32; HARMONY_VOICES], [Q12; HARMONY_VOICES], usize) {
    let mut tones = [0; HARMONY_VOICES];
    let mut gains = [-Q12::ONE; HARMONY_VOICES];
    let count = match config.source {
        HarmonySource::ChordByDegree => {
            let chord = DEGREE[config.degree as usize];
            for (tone, &degree) in tones.iter_mut().zip(chord) {
                *tone = degree / 7 * 12 + SCALE_TONES[config.scale as usize][(degree % 7) as usize];
            }
            chord.len()
        }
        HarmonySource::ChordByName => {
            let chord = NAMED[config.chord as usize];
            tones[..chord.len()].copy_from_slice(chord);
            chord.len()
        }
        HarmonySource::ChordViaMidi => {
            let mut count = 0;
            for voice in 0..HARMONY_VOICES {
                if midi_period[voice] != Q6::ZERO {
                    tones[count] = midi_note[voice] + 72;
                    gains[count] = midi_gain[voice];
                    count += 1;
                }
            }
            sort(&mut tones, &mut gains, count);
            count
        }
        _ => 0,
    };
    if count == 0 {
        return (tones, gains, 0);
    }
    if matches!(
        config.source,
        HarmonySource::ChordByDegree | HarmonySource::ChordByName
    ) {
        for _ in 0..config.inversion as u8 {
            tones[..count].rotate_left(1);
            tones[count - 1] += 12;
        }
        for tone in &mut tones[..count] {
            *tone += i32::from(config.key.semitone());
        }
    }
    let low = (tones[0] + 24) / 12;
    let high = (21 - tones[count - 1]) / 12;
    let register = f64::from(config.register_percent as i32) / 100.0;
    let shift =
        (-(f64::from(low) + 0.49) * (1.0 - register) + (f64::from(high) + 0.49) * register) as f32;
    let shift = shift.round() as i32;
    for tone in &mut tones[..count] {
        *tone += shift * 12;
    }
    let spread = f64::from(config.voicing_spread_percent as i32) / 100.0;
    let top = ((f64::from(tones[count - 1]) + 0.49) * (1.0 - spread) + spread * 21.49) as f32;
    let max_note = top.round() as i32;
    let mut best = [0; HARMONY_VOICES];
    let mut best_score = 0;
    let candidates = if count > 1 {
        4usize.pow((count - 1) as u32)
    } else {
        0
    };
    for candidate in 0..candidates {
        let mut offsets = [0; HARMONY_VOICES];
        let mut encoded = candidate;
        for offset in offsets[1..count].iter_mut().rev() {
            *offset = (encoded % 4) as i32;
            encoded /= 4;
        }
        let mut sorted = std::array::from_fn::<_, 4, _>(|i| tones[i] + offsets[i] * 12);
        sorted[..count].sort_unstable();
        let mut score = 0;
        if sorted[count - 1] <= max_note {
            score = (sorted[count - 1] + 8 - max_note).max(0) * 100;
            if count > 2 && sorted[1] - sorted[0] > sorted[2] - sorted[1] {
                score += 60;
            }
            if count == 4 && sorted[2] - sorted[1] > sorted[3] - sorted[2] {
                score += 40;
            }
        }
        if score > best_score {
            best_score = score;
            best = offsets;
        }
    }
    for i in 0..count {
        tones[i] += best[i] * 12;
    }
    sort(&mut tones, &mut gains, count);
    (tones, gains, count)
}

/// Sorts the active tones at the start of the array and moves each gain with
/// its tone.
/// `count` must be at most `HARMONY_VOICES`. The order of the pairwise swaps is
/// important for equal tones with different gains. Do not replace it with an
/// arbitrary unstable permutation.
fn sort(tones: &mut [i32; HARMONY_VOICES], gains: &mut [Q12; HARMONY_VOICES], count: usize) {
    for i in 0..count.saturating_sub(1) {
        for j in i + 1..count {
            if tones[i] > tones[j] {
                tones.swap(i, j);
                gains.swap(i, j);
            }
        }
    }
}
