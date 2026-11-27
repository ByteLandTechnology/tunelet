//! Optional stages for dynamics, saturation, reverse loops and reverberation.
//!
//! Punch and Warm operate on mono PCM before resynthesis. The processor does
//! not call their sample methods while they are disabled. Alien and reverb
//! accept stereo Q14 frames after resynthesis. They have their own bypass
//! transitions. Alien captures a fresh loop when you enable it, and ramps its
//! wet mix on each toggle. Reverb lets an active tail decay with zero input
//! during bypass, then clears its histories.
//!
//! The configuration types validate the public controls. The prepared stage
//! types assume that these checks were successful.

pub(crate) mod alien;
pub(crate) mod punch;
pub(crate) mod reverb;
pub(crate) mod warm;
