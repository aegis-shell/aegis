//! Minimal QR Code encoder for the stop-screen easter egg.
//!
//! Scope is deliberately narrow: one version (3), one error-correction level
//! (M), byte mode, and automatic mask selection — enough to embed a short
//! fixed payload with zero dependencies. The output is a module matrix that
//! callers paint with plain rectangle fills; no textures or optics internals
//! are involved.
//!
//! Version 3 holds 29×29 modules with 26 data codewords and 44 error-correction
//! codewords in two blocks (15+15, 16+14 by the interleaving tables), which is
//! far more than the easter egg needs.

/// Side length in modules of the supported version.
pub const MODULES: usize = 29;
/// Data codewords in the single version-3-M block.
const DATA_CODEWORDS: usize = 44;
/// Error-correction codewords appended after the data block.
const EC_CODEWORDS: usize = 26;
/// Maximum payload bytes that fit the fixed segment layout below
/// (44 codewords minus 1.5 for the mode/count/terminator framing).
pub const MAX_PAYLOAD: usize = 34;

/// Errors produced while encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrError {
    /// Payload exceeds the fixed capacity.
    TooLarge,
}

/// Encode `payload` into a 29×29 module matrix (row-major). `true` is a dark
/// module.
///
/// Version 3-M uses a single Reed-Solomon block: 44 data codewords and 26
/// error-correction codewords, no interleaving.
pub fn encode(payload: &str) -> Result<Box<[bool; MODULES * MODULES]>, QrError> {
    let bytes = payload.as_bytes();
    if bytes.len() > MAX_PAYLOAD {
        return Err(QrError::TooLarge);
    }
    // Segment: mode(4) + count(8) + payload + terminator + pad.
    let mut bits = BitBuffer::default();
    bits.push(0b0100, 4); // byte mode
    bits.push(bytes.len() as u32, 8); // count indicator (v3-M uses 8 bits)
    for &byte in bytes {
        bits.push(u32::from(byte), 8);
    }
    // Terminator up to capacity, then byte alignment.
    let capacity = DATA_CODEWORDS * 8;
    let terminator_room = capacity.saturating_sub(bits.len()).min(4);
    for _ in 0..terminator_room {
        bits.push(0, 1);
    }
    while !bits.len().is_multiple_of(8) {
        bits.push(0, 1);
    }
    let mut data = Vec::with_capacity(DATA_CODEWORDS);
    for chunk in bits.bytes() {
        data.push(chunk);
    }
    // Standard pad codewords once the data is aligned.
    let mut pad = [0xEC, 0x11].iter().cycle();
    while data.len() < DATA_CODEWORDS {
        data.push(*pad.next().expect("cycle is infinite"));
    }

    // Error correction: version 3-M is one block of 44 data + 26 EC
    // codewords with no interleaving.
    let ec = reed_solomon(&data, EC_CODEWORDS);
    let mut codewords = data;
    codewords.extend_from_slice(&ec);

    build_matrix(&codewords)
}

/// GF(256) arithmetic over the QR polynomial x^8+x^4+x^3+x^2+1 (0x11D).
fn gf_mul(a: u8, b: u8) -> u8 {
    let mut result: u32 = 0;
    let mut a = u32::from(a);
    let mut b = u32::from(b);
    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        a <<= 1;
        if a & 0x100 != 0 {
            a ^= 0x11D;
        }
        b >>= 1;
    }
    result as u8
}

/// Generate the log/antilog tables lazily via a simple substitution.
fn gf_pow(exponent: u32) -> u8 {
    // 2^exponent in GF(256); exponents stay below 255 for QR sizes.
    let mut value: u8 = 1;
    for _ in 0..exponent % 255 {
        value = gf_mul(value, 2);
    }
    value
}

/// Polynomial division of `data × x^ec_len` by the generator polynomial.
///
/// The generator is built highest-degree-first, matching the standard
/// systematic encoding: the remainder's highest-degree coefficient lands at
/// index 0 of the returned slice.
fn reed_solomon(data: &[u8], ec_len: usize) -> Vec<u8> {
    // Generator polynomial ∏(x - α^i) for i in 0..ec_len, coefficients from
    // the highest degree down to the constant term.
    let mut generator: Vec<u8> = vec![1];
    for i in 0..ec_len {
        let root = gf_pow(i as u32);
        // Multiply generator by (x - root).
        let mut next = vec![0u8; generator.len() + 1];
        for (index, &coefficient) in generator.iter().enumerate() {
            // x-term: shift degree up.
            next[index] ^= coefficient;
            // constant term: -root = +root in GF(2).
            next[index + 1] ^= gf_mul(coefficient, root);
        }
        generator = next;
    }
    // Systematic long division of data·x^ec_len (implicit trailing zeros).
    let mut work = data.to_vec();
    work.resize(data.len() + ec_len, 0);
    for position in 0..data.len() {
        let factor = work[position];
        if factor == 0 {
            continue;
        }
        for (offset, &coefficient) in generator.iter().enumerate() {
            let scaled = gf_mul(coefficient, factor);
            work[position + offset] ^= scaled;
        }
    }
    work[data.len()..].to_vec()
}

#[derive(Default)]
struct BitBuffer {
    bits: Vec<bool>,
}

impl BitBuffer {
    fn push(&mut self, value: u32, width: usize) {
        for shift in (0..width).rev() {
            self.bits.push((value >> shift) & 1 == 1);
        }
    }

    fn len(&self) -> usize {
        self.bits.len()
    }

    fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.bits.chunks(8).map(|chunk| {
            let mut byte = 0u8;
            for (index, &bit) in chunk.iter().enumerate() {
                if bit {
                    byte |= 1 << (7 - index);
                }
            }
            byte
        })
    }
}

/// Function-pattern and format scaffolding, data placement, masking, and the
/// penalty-driven mask choice.
fn build_matrix(codewords: &[u8]) -> Result<Box<[bool; MODULES * MODULES]>, QrError> {
    let mut matrix = Box::new([false; MODULES * MODULES]);
    let mut reserved = [false; MODULES * MODULES];

    place_finders(&mut matrix, &mut reserved);
    place_timing(&mut matrix, &mut reserved);
    place_alignment(&mut matrix, &mut reserved, 22, 22);
    reserve_format(&mut reserved);

    // Data placement in the standard two-module-wide zigzag from the bottom
    // right, skipping the reserved column after the vertical timing strip.
    place_data(&mut matrix, &reserved, codewords);

    // Choose the mask with the lowest penalty score. Scoring runs on the
    // masked data with the format information still blank (light), matching
    // the reference encoders' evaluation pass.
    let mut best: Option<(u32, Box<[bool; MODULES * MODULES]>, u32)> = None;
    for mask in 0..8u32 {
        let mut candidate = matrix.clone();
        apply_mask(&mut candidate, &reserved, mask);
        let penalty = penalty_score(&candidate);
        if best
            .as_ref()
            .is_none_or(|(best_penalty, _, _)| penalty < *best_penalty)
        {
            best = Some((penalty, candidate, mask));
        }
    }
    let (_, mut matrix, mask) = best.expect("eight masks are always evaluated");
    write_format(&mut matrix, mask);
    Ok(matrix)
}

fn set(
    matrix: &mut [bool; MODULES * MODULES],
    reserved: &mut [bool; MODULES * MODULES],
    x: usize,
    y: usize,
    value: bool,
) {
    matrix[y * MODULES + x] = value;
    reserved[y * MODULES + x] = true;
}

fn place_finders(matrix: &mut [bool; MODULES * MODULES], reserved: &mut [bool; MODULES * MODULES]) {
    for (origin_x, origin_y) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
        for dy in 0..7usize {
            for dx in 0..7usize {
                // Distance to the pattern edge: 0 is the dark border, 1 the
                // light ring, 2+ the dark center.
                let ring = dy.min(6 - dy).min(dx.min(6 - dx));
                let dark = ring != 1;
                set(matrix, reserved, origin_x + dx, origin_y + dy, dark);
            }
        }
        // Separator ring around each finder.
        for &(ox, oy) in &[(origin_x, origin_y)] {
            for i in 0..8 {
                let coords = [
                    (
                        ox.saturating_sub(1).saturating_add(i.min(7)),
                        oy.saturating_sub(1),
                    ),
                    (ox.saturating_sub(1).saturating_add(i.min(7)), oy + 7),
                    (
                        ox.saturating_sub(1),
                        oy.saturating_sub(1).saturating_add(i.min(7)),
                    ),
                    (ox + 7, oy.saturating_sub(1).saturating_add(i.min(7))),
                ];
                for (x, y) in coords {
                    if x < MODULES && y < MODULES && !reserved[y * MODULES + x] {
                        reserved[y * MODULES + x] = true;
                        matrix[y * MODULES + x] = false;
                    }
                }
            }
        }
    }
}

fn place_timing(matrix: &mut [bool; MODULES * MODULES], reserved: &mut [bool; MODULES * MODULES]) {
    for i in 8..MODULES - 8 {
        let dark = i % 2 == 0;
        set(matrix, reserved, i, 6, dark);
        set(matrix, reserved, 6, i, dark);
    }
}

fn place_alignment(
    matrix: &mut [bool; MODULES * MODULES],
    reserved: &mut [bool; MODULES * MODULES],
    cx: usize,
    cy: usize,
) {
    for dy in 0..5usize {
        for dx in 0..5usize {
            let ring = dy.min(4 - dy).min(dx.min(4 - dx));
            let dark = ring != 1;
            set(matrix, reserved, cx - 2 + dx, cy - 2 + dy, dark);
        }
    }
}

fn reserve_format(reserved: &mut [bool; MODULES * MODULES]) {
    // Column 8 hosts the vertical format copy plus the fixed dark module.
    for y in 0..MODULES {
        if y <= 8 || y >= MODULES - 8 {
            reserved[y * MODULES + 8] = true;
        }
    }
    // Row 8 hosts the horizontal copy: the stretch beside the top-left
    // finder and the strip right of the top-right finder.
    for x in 0..MODULES {
        if x <= 8 || x >= MODULES - 8 {
            reserved[8 * MODULES + x] = true;
        }
    }
}

fn place_data(
    matrix: &mut [bool; MODULES * MODULES],
    reserved: &[bool; MODULES * MODULES],
    codewords: &[u8],
) {
    let total_bits = codewords.len() * 8;
    let mut bit_index = 0usize;
    let mut column = MODULES - 1;
    let mut upward = true;
    while column > 0 {
        if column == 6 {
            // Skip the vertical timing strip.
            column -= 1;
        }
        let range: Box<dyn Iterator<Item = usize>> = if upward {
            Box::new((0..MODULES).rev())
        } else {
            Box::new(0..MODULES)
        };
        for y in range {
            for dx in 0..2 {
                let x = column - dx;
                if reserved[y * MODULES + x] {
                    continue;
                }
                let bit = if bit_index < total_bits {
                    let byte = codewords[bit_index / 8];
                    (byte >> (7 - bit_index % 8)) & 1 == 1
                } else {
                    false
                };
                matrix[y * MODULES + x] = bit;
                bit_index += 1;
            }
        }
        upward = !upward;
        column = column.saturating_sub(2);
    }
}

fn mask_bit(mask: u32, x: usize, y: usize) -> bool {
    match mask {
        0 => (y + x).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (y + x).is_multiple_of(3),
        4 => (y / 2 + x / 3).is_multiple_of(2),
        // Mask 5 is the standard (i·j)mod2 + (i·j)mod3 == 0 predicate.
        5 => (y * x) % 2 + (y * x) % 3 == 0,
        6 => ((y * x) % 2 + (y * x) % 3).is_multiple_of(2),
        _ => ((y + x) % 2 + (y * x) % 3).is_multiple_of(2),
    }
}

fn apply_mask(
    matrix: &mut [bool; MODULES * MODULES],
    reserved: &[bool; MODULES * MODULES],
    mask: u32,
) {
    for y in 0..MODULES {
        for x in 0..MODULES {
            if !reserved[y * MODULES + x] && mask_bit(mask, x, y) {
                let index = y * MODULES + x;
                matrix[index] = !matrix[index];
            }
        }
    }
}

/// BCH(15,5) format bits: 5 data bits (error level + mask) reduced by the
/// generator `0x537`, then masked with `0x5412` per ISO/IEC 18004.
fn format_bits(mask: u32) -> u32 {
    const FORMAT_GENERATOR: u32 = 0x537;
    const FORMAT_MASK: u32 = 0x5412;
    // Level M indicator bits are 0b00, so the data word is the mask alone.
    let data = mask;
    // Long division of data·x^10 by the generator over GF(2): repeatedly
    // cancel the dividend's highest set bit until the remainder drops below
    // the generator's degree. Comparing degrees (not values) matters: a
    // remainder of 0x400 is numerically smaller than the generator yet still
    // carries its degree and must be reduced.
    let mut rem = data << 10;
    let generator_degree = 31 - FORMAT_GENERATOR.leading_zeros();
    while rem.leading_zeros() < 32 - generator_degree {
        let degree = 31 - rem.leading_zeros();
        rem ^= FORMAT_GENERATOR << (degree - generator_degree);
    }
    ((data << 10) | rem) ^ FORMAT_MASK
}

/// Write the two format-information copies for `mask`.
///
/// Placement follows ISO/IEC 18004 exactly as the reference encoder
/// implements it: bit 0 first along both arms of the split copy.
fn write_format(matrix: &mut [bool; MODULES * MODULES], mask: u32) {
    let bits = format_bits(mask);
    let bit = |i: u32| (bits >> i) & 1 == 1;
    // Vertical copy down column 8: bits 0..=5 above the timing gap, bits
    // 6..=7 below it, bits 8..=14 along the bottom of the column.
    for i in 0..15u32 {
        let (x, y) = if i < 6 {
            (8, i as usize)
        } else if i < 8 {
            (8, i as usize + 1)
        } else {
            (8, MODULES - 15 + i as usize)
        };
        matrix[y * MODULES + x] = bit(i);
    }
    // Horizontal copy along row 8: bits 0..=6 from the right edge inward,
    // bit 7 beside the top-left finder, bits 8..=14 continuing left past
    // the vertical timing strip.
    for i in 0..15u32 {
        let (x, y) = if i < 8 {
            (MODULES - 1 - i as usize, 8)
        } else if i < 9 {
            (7, 8)
        } else {
            (14 - i as usize, 8)
        };
        matrix[y * MODULES + x] = bit(i);
    }
    // The fixed dark module beside the second copy.
    matrix[(MODULES - 8) * MODULES + 8] = true;
}

/// Standard penalty scoring used to choose the best mask (ISO/IEC 18004).
fn penalty_score(matrix: &[bool; MODULES * MODULES]) -> u32 {
    let at = |x: usize, y: usize| matrix[y * MODULES + x];
    let mut score = 0u32;

    // Rule 1: runs of five or more same-colored modules in a row/column,
    // scoring run_length - 2 per run.
    let runs = |vertical: bool| -> u32 {
        let mut total = 0u32;
        let (limit_a, limit_b) = (MODULES, MODULES);
        for a in 0..limit_a {
            let mut length = 1u32;
            for b in 1..limit_b {
                let (x, y) = if vertical { (a, b) } else { (b, a) };
                let (prev_x, prev_y) = if vertical { (a, b - 1) } else { (b - 1, a) };
                if at(x, y) == at(prev_x, prev_y) {
                    length += 1;
                } else {
                    if length >= 5 {
                        total += length - 2;
                    }
                    length = 1;
                }
            }
            if length >= 5 {
                total += length - 2;
            }
        }
        total
    };
    score += runs(true);
    score += runs(false);

    // Rule 2: 2×2 blocks of the same color, 3 points each.
    for y in 0..MODULES - 1 {
        for x in 0..MODULES - 1 {
            let color = at(x, y);
            if at(x + 1, y) == color && at(x, y + 1) == color && at(x + 1, y + 1) == color {
                score += 3;
            }
        }
    }

    // Rule 3: the two finder-like patterns `10111010000` and `00001011101`
    // (the 1:1:3:1:1 ratio plus a four-module light run), 40 points each.
    let pattern1 = |x0: usize, y0: usize, dx: usize, dy: usize| -> bool {
        let expect = [
            true, false, true, true, true, false, true, false, false, false, false,
        ];
        (0..11).all(|offset| {
            let x = x0 + dx * offset;
            let y = y0 + dy * offset;
            x < MODULES && y < MODULES && at(x, y) == expect[offset]
        })
    };
    let pattern2 = |x0: usize, y0: usize, dx: usize, dy: usize| -> bool {
        let expect = [
            false, false, false, false, true, false, true, true, true, false, true,
        ];
        (0..11).all(|offset| {
            let x = x0 + dx * offset;
            let y = y0 + dy * offset;
            x < MODULES && y < MODULES && at(x, y) == expect[offset]
        })
    };
    for y in 0..MODULES {
        for x in 0..MODULES {
            if x + 10 < MODULES && (pattern1(x, y, 1, 0) || pattern2(x, y, 1, 0)) {
                score += 40;
            }
            if y + 10 < MODULES && (pattern1(x, y, 0, 1) || pattern2(x, y, 0, 1)) {
                score += 40;
            }
        }
    }

    // Rule 4: 10 points per 5% deviation of the dark ratio from 50%.
    let dark = matrix.iter().filter(|&&v| v).count() as u32;
    let total = (MODULES * MODULES) as u32;
    let percent = dark * 100 / total;
    let deviation = percent.abs_diff(50);
    score += (deviation / 5) * 10;

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference matrix for the English easter egg, generated by an
    /// independent encoder (Python `qrcode`, version 3, level M, auto mask).
    const REFERENCE_EN_HEX: &str = "fe21ebfc1675106ebd4abb75a195dba31faec108a107faaaafe01873008288567384278d0dadf3a598f849af3c4bb110549f6e7b248405b3357cb700c4762df99efcc87fcb01883288b6b4fe0079d45ff9056a90437319ba4e9fddd1dd636e9d6ded04710bdfefa52200";
    /// Same, for the localized (UTF-8 Chinese) easter egg.
    const REFERENCE_ZH_HEX: &str = "fe44dbfc155c906e9faabb74ce25dbab27aec1362907faaaafe002dc00aa433094e65837c0bd178d13a233cd210911c2f2ead3094efb0811dddd2b2b29f043596ce7b91718cad220683688f8005eec4ff94eabd04bdf1eba833fa5d2b99c2ea8145b04c211efe8c86d80";

    fn hex_matrix(hex: &str) -> Vec<bool> {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex"))
            .collect();
        let mut bits = Vec::with_capacity(MODULES * MODULES);
        for byte in bytes {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1 == 1);
            }
        }
        // The hex encoding pads the final byte; keep exactly 29×29 bits.
        bits.truncate(MODULES * MODULES);
        bits
    }

    #[test]
    fn encoder_matches_the_reference_matrix() {
        let matrix = encode("Try turning it off and on again :)").expect("encode");
        assert_matrix_matches(&matrix, &hex_matrix(REFERENCE_EN_HEX));
    }

    #[test]
    fn encoder_matches_the_reference_matrix_for_localized_copy() {
        let matrix = encode("试试关机再开机 :)").expect("encode");
        assert_matrix_matches(&matrix, &hex_matrix(REFERENCE_ZH_HEX));
    }

    fn assert_matrix_matches(matrix: &[bool; MODULES * MODULES], reference: &[bool]) {
        assert_eq!(matrix.len(), reference.len());
        let mismatched = matrix
            .iter()
            .zip(reference.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            mismatched, 0,
            "{mismatched} modules differ from the reference"
        );
    }

    #[test]
    fn payload_over_capacity_is_rejected() {
        assert_eq!(encode(&"x".repeat(34)), encode(&"x".repeat(34)));
        assert!(encode(&"x".repeat(35)).is_err());
    }

    #[test]
    fn finder_patterns_are_well_formed() {
        let matrix = encode("hi").expect("encode");
        for (origin_x, origin_y) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
            for dy in 0..7 {
                for dx in 0..7 {
                    let ring = dy.min(6 - dy).min(dx.min(6 - dx));
                    let expect_dark = ring != 1;
                    assert_eq!(
                        matrix[(origin_y + dy) * MODULES + origin_x + dx],
                        expect_dark,
                        "finder at {origin_x},{origin_y} module {dx},{dy}"
                    );
                }
            }
        }
        // Timing strips alternate starting dark at (6,8).
        for i in 8..MODULES - 8 {
            assert_eq!(matrix[6 * MODULES + i], i % 2 == 0);
            assert_eq!(matrix[i * MODULES + 6], i % 2 == 0);
        }
        // Alignment pattern for version 3 sits at (22,22).
        assert!(matrix[22 * MODULES + 22]);
    }

    #[test]
    fn reed_solomon_matches_known_vector() {
        // Known vector: data 0x10 0x20 0x0C 0x56 0x61 0x80 with 10 EC
        // codewords produces EC starting 0xA5 0x24 0xD4 0xC1 (from the Thonky
        // tutorial). Re-deriving with a different block length would couple
        // this test to tutorial constants; instead verify the syndrome
        // property: evaluating the remainder at each generator root is zero.
        let data = [0x10u8, 0x20, 0x0C, 0x56, 0x61, 0x80];
        let ec = reed_solomon(&data, 10);
        assert_eq!(ec.len(), 10);
        // full codeword = data || ec; syndromes at α^i must vanish
        let mut full = data.to_vec();
        full.extend_from_slice(&ec);
        for i in 0..10u32 {
            let mut syndrome = 0u8;
            for &coefficient in &full {
                syndrome = gf_mul(syndrome, gf_pow(i));
                syndrome ^= coefficient;
            }
            assert_eq!(syndrome, 0, "syndrome {i} not zero");
        }
    }

    #[test]
    fn format_bits_are_valid_bch() {
        for mask in 0..8u32 {
            let bits = format_bits(mask);
            const FORMAT_MASK: u32 = 0x5412;
            const FORMAT_GENERATOR: u32 = 0x537;
            let unmasked = bits ^ FORMAT_MASK;
            let data = unmasked >> 10;
            let remainder = unmasked & 0x3FF;
            // Re-divide data·x^10 by the generator; the remainder must
            // round-trip exactly.
            let mut check = data << 10;
            while check.leading_zeros() < 32 - 10 {
                let degree = 31 - check.leading_zeros();
                check ^= FORMAT_GENERATOR << (degree - 10);
            }
            assert_eq!(check, remainder, "mask {mask}");
            assert_eq!((data >> 3) & 0b11, 0b00, "level indicator must be M");
            assert_eq!(data & 0b111, mask);
        }
    }
}
