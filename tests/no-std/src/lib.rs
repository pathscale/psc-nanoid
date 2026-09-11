#![no_std]

use psc_nanoid::{nanoid, Nanoid};
use psc_nanoid::alphabet::Base64UrlAlphabet;
use psc_nanoid::packed::PackedNanoid;

pub const FIXED: Nanoid<5> = nanoid!("abc12");

/// Caller-supplied entropy remains available without a thread-local RNG.
pub fn generate(rng: impl rand::Rng) -> PackedNanoid<21, 16, Base64UrlAlphabet> {
    PackedNanoid::pack(&Nanoid::new_with(rng)).expect("the default alphabet fits")
}
