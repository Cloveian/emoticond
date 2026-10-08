//! The `.kmj` data file, format 3: constants and record layouts.
//!
//! docs/format.md is the normative description; this module is its code.
//! The reader ([`DataFile`](super::DataFile)) and the compiler
//! (`emoticond-compile`) share these definitions, so the two cannot drift.
//!
//! All integers and floats are little-endian. Records are `repr(C)` structs
//! of 4-byte fields with no padding, so a section is read in place as a
//! slice of records (zero copy) on little-endian hosts.

/// The first 8 bytes of every data file.
pub const MAGIC: [u8; 8] = *b"KAOMOJI\x1a";
/// The format major this library reads. Other majors are refused.
pub const FORMAT_MAJOR: u16 = 3;
/// The newest format minor this library writes. Newer minors are read
/// (unknown optional sections are skipped). A writer records minor 0 when a
/// file uses only format 3.0 sections, so 3.0 readers read it; any 3.1
/// section (the compact encodings, `ALIA`, `RETD`) makes it 3.1.
pub const FORMAT_MINOR: u16 = 1;
/// Size of the fixed header, before the section directory.
pub const HEADER_LEN: usize = 64;
/// Size of one section directory entry.
pub const DIR_ENTRY_LEN: usize = 32;
/// Most sections a file may have (a sanity bound for the reader).
pub const MAX_SECTIONS: usize = 4096;

/// Header flag: built under a public licence policy.
pub const FLAG_PUBLIC: u32 = 1 << 0;
/// Header flag: has an alias table (`ALIA`).
pub const FLAG_ALIASES: u32 = 1 << 1;

/// Directory entry flag: a reader that does not know this section must
/// refuse the file.
pub const SECTION_REQUIRED: u32 = 1 << 0;

/// "No face" in a neighbour list or canonical record.
pub const NONE: u32 = u32::MAX;

/// A section's four-byte tag.
pub type Tag = [u8; 4];

/// Every section this library knows, with its element size and whether a
/// file must have it. `GRAM` may occur more than once (one per language);
/// every other tag at most once.
pub const SECTIONS: &[(Tag, &str, usize, bool)] = &[
    (*b"MANI", "MANI", 1, true),
    (*b"LICN", "LICN", 1, true),
    (*b"ATTR", "ATTR", 1, true),
    (*b"GRAM", "GRAM", 1, true),
    (*b"STRS", "STRS", 1, true),
    (*b"FNUM", "FNUM", std::mem::size_of::<FaceNum>(), true),
    (*b"FIDS", "FIDS", 8, true),
    (*b"FTXT", "FTXT", 8, true),
    (*b"FCHR", "FCHR", 8, true),
    (*b"CHRS", "CHRS", 4, true),
    (*b"BTXT", "BTXT", 4, true),
    (*b"TERM", "TERM", std::mem::size_of::<TermRec>(), true),
    (*b"TKEY", "TKEY", 8, true),
    (*b"TSRT", "TSRT", 4, true),
    (*b"DNSE", "DNSE", 4, true),
    (*b"ENGN", "ENGN", 4, false),
    (*b"WKEY", "WKEY", 8, true),
    (*b"WPST", "WPST", 8, true),
    (*b"POST", "POST", 4, true),
    (*b"SLST", "SLST", 8, true),
    (*b"PHRS", "PHRS", std::mem::size_of::<PhraseRec>(), true),
    (*b"PIDX", "PIDX", 12, true),
    (*b"SITU", "SITU", std::mem::size_of::<SituRec>(), true),
    (*b"SMAT", "SMAT", 8, true),
    (*b"CANO", "CANO", std::mem::size_of::<CanonRec>(), true),
    (*b"BOST", "BOST", std::mem::size_of::<BoostRec>(), false),
    // format 3.1: compact encodings. Each replaces a 3.0 section (`FNUM`,
    // `DNSE`, `ENGN`, `POST`), so a file has one or the other; they are
    // flagged required, so a 3.0 reader refuses such a file as
    // `Incompatible` instead of reading it wrongly.
    (*b"FNQ2", "FNQ2", FACE_Q2_LEN, true),
    (*b"FNQ1", "FNQ1", FACE_Q1_LEN, true),
    (*b"FNCB", "FNCB", 4, true),
    (*b"DNSP", "DNSP", 4, true),
    (*b"ENGP", "ENGP", 4, true),
    (*b"PSTV", "PSTV", 1, true),
    // format 3.1: ids across releases (optional: a reader without them
    // simply does not resolve old ids)
    (*b"ALIA", "ALIA", 16, false),
    (*b"RETD", "RETD", 16, false),
];

/// Tags that exist only in format 3.1 and later: a file with any of them is
/// written as minor 1.
pub const SECTIONS_3_1: &[Tag] = &[*b"FNQ2", *b"FNQ1", *b"FNCB", *b"DNSP", *b"ENGP", *b"PSTV", *b"ALIA", *b"RETD"];

/// The numbers of a face in a coded record (`FNQ2`, `FNQ1`), in this order:
/// `r[0..19]`, `multi`, `cute`, `intensity`, `suggestive`, `lenny`, `face`,
/// `quality`, `crude`. `len` is stored apart, as a u16.
pub const N_CODED: usize = N_EMO + 8;
/// `FNQ2` record: `N_CODED` u16 codes, then `len: u16`.
pub const FACE_Q2_LEN: usize = 2 * N_CODED + 2;
/// `FNQ1` record: `N_CODED` u8 codes, a zero byte, then `len: u16`.
pub const FACE_Q1_LEN: usize = N_CODED + 1 + 2;

impl FaceNum {
    /// The coded fields in `FNQ*` order.
    pub fn coded(&self) -> [f32; N_CODED] {
        let mut v = [0f32; N_CODED];
        v[..N_EMO].copy_from_slice(&self.r);
        v[N_EMO..].copy_from_slice(&[
            self.multi,
            self.cute,
            self.intensity,
            self.suggestive,
            self.lenny,
            self.face,
            self.quality,
            self.crude,
        ]);
        v
    }

    /// The inverse of [`coded`](FaceNum::coded).
    pub fn from_coded(v: &[f32; N_CODED], len: u32) -> FaceNum {
        let mut r = [0f32; N_EMO];
        r.copy_from_slice(&v[..N_EMO]);
        let x = &v[N_EMO..];
        FaceNum {
            r,
            multi: x[0],
            cute: x[1],
            intensity: x[2],
            suggestive: x[3],
            lenny: x[4],
            face: x[5],
            quality: x[6],
            crude: x[7],
            len,
            flags: 0,
        }
    }
}

/// `RETD` reasons.
pub const RETIRED_OTHER: u32 = 0;
pub const RETIRED_PRUNED: u32 = 1;
pub const RETIRED_LICENCE: u32 = 2;
pub const RETIRED_BLOCKLISTED: u32 = 3;
pub const RETIRED_QUALITY: u32 = 4;

/// Packed neighbour lists (`DNSP`, `ENGP`), as u32 words: `bits`, `n_terms`,
/// then `n_terms + 1` start slots, then the slots, `bits` wide each, packed
/// little-endian from bit 0 of the first word after the starts. Term `t`
/// has slots `start[t]..start[t + 1]` (at most `dense_k`; trailing empty
/// slots are not stored). A slot holds an ordinal, or all ones for none.
pub const PACKED_HEAD: usize = 2;

/// Number of emotions in a profile (`FaceNum::r`, `TermRec::p`, ...). The
/// manifest names them; their count is part of the record layout.
pub const N_EMO: usize = 19;

/// A face's numbers (`FNUM`, one per face, in `FIDS` order).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceNum {
    /// Emotion intensities 0..1, in the manifest's `emotions` order.
    pub r: [f32; N_EMO],
    /// 0..1: more than one figure.
    pub multi: f32,
    pub cute: f32,
    pub intensity: f32,
    /// 0..3: the suggestive scale (`safety.*_at` thresholds in the manifest).
    pub suggestive: f32,
    pub lenny: f32,
    /// 0..1: how much this reads as a face at all.
    pub face: f32,
    /// Predicted visual quality, 1..8.
    pub quality: f32,
    /// 0..1 crude score (`crude.at` threshold in the manifest).
    pub crude: f32,
    /// Length in chars.
    pub len: u32,
    /// Reserved, 0.
    pub flags: u32,
}

/// `TermRec::flags`: the profile comes only from e5's neighbours.
pub const TERM_WEAK: u32 = 1 << 0;
/// `TermRec::flags`: `m` is known.
pub const TERM_HAS_M: u32 = 1 << 1;

/// A vocab term's numbers (`TERM`, one per vocab line).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TermRec {
    /// Emotion profile, summing to 1 (or all 0).
    pub p: [f32; N_EMO],
    /// Expected share of multi-figure faces (valid with `TERM_HAS_M`).
    pub m: f32,
    /// Faces carrying the term.
    pub n: u32,
    /// 2 = an emotion word, 1 = a hand-written lexicon key, 0 = a tag phrase.
    pub tier: u32,
    /// `TERM_WEAK`, `TERM_HAS_M`.
    pub flags: u32,
}

impl TermRec {
    pub fn weak(&self) -> bool {
        self.flags & TERM_WEAK != 0
    }
    /// `m`, when known.
    pub fn m(&self) -> Option<f32> {
        (self.flags & TERM_HAS_M != 0).then_some(self.m)
    }
}

/// `PhraseRec::level` / `pair`: not given.
pub const UNSET: u32 = u32::MAX;
/// `PhraseRec::flags`.
pub const PHRASE_CUTE: u32 = 1 << 0;
pub const PHRASE_LENNY: u32 = 1 << 1;
pub const PHRASE_LEWD: u32 = 1 << 2;
/// The phrase beats a vocab term of the same spelling however well the data
/// knows the word (hand-written entries, `"over": true`).
pub const PHRASE_OVER: u32 = 1 << 3;

/// A phrase of the generated search lexicon (`PHRS`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhraseRec {
    pub p: [f32; N_EMO],
    /// Span of the key in `STRS`.
    pub key: [u32; 2],
    /// Range of its tag words in `SLST`.
    pub words: [u32; 2],
    /// 1 mild, 2 plain, 3 strong; `UNSET` when not given.
    pub level: u32,
    /// 0 single, 1 pair; `UNSET` when not given.
    pub pair: u32,
    /// `PHRASE_*` flags.
    pub flags: u32,
}

/// A hand-written situation (`SITU`), in name order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SituRec {
    pub p: [f32; N_EMO],
    /// Span of the name in `STRS`.
    pub name: [u32; 2],
    /// Range of its match spellings in `SMAT`.
    pub matches: [u32; 2],
    /// Range of its tag words in `SLST`.
    pub words: [u32; 2],
    /// 0 single, 1 pair; `UNSET` when not given.
    pub pair: u32,
}

/// One concept's canonical picks (`CANO`), sorted by term.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanonRec {
    /// Span of the term in `STRS`.
    pub term: [u32; 2],
    /// Face ordinals, best first; `NONE` after the last.
    pub faces: [u32; 3],
    /// How many of `faces` are set (1..=3).
    pub n: u32,
}

/// A curated boost (`BOST`), sorted by (term, id).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoostRec {
    /// Span of the term in `STRS`.
    pub term: [u32; 2],
    /// The face id's 48-bit value, low then high 32 bits.
    pub id: [u32; 2],
    pub boost: f32,
    /// Reserved, 0.
    pub reserved: u32,
}

/// Append `v` as an LEB128 varint (7 bits a byte, low first).
pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Read an LEB128 varint at `*pos`, advancing it. `None` at the end of the
/// bytes or for a varint longer than 10 bytes (damage): never panics.
pub fn get_varint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= u64::from(byte & 0x7f).checked_shl(shift).unwrap_or(0);
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// The width of a packed slot for `n` faces: every ordinal and the
/// all-ones "none" code fit.
pub fn packed_bits(n: usize) -> u32 {
    (usize::BITS - n.leading_zeros()).max(1)
}

/// Pack `slots` (`bits` wide each, `NONE` stored as all ones) into u32 words.
pub fn pack_slots(slots: &[u32], bits: u32) -> Vec<u32> {
    let mask = if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 };
    let total = slots.len() as u64 * u64::from(bits);
    let mut w = vec![0u32; total.div_ceil(32) as usize];
    for (j, &s) in slots.iter().enumerate() {
        let v = u64::from(s.min(mask) & mask);
        let at = j as u64 * u64::from(bits);
        let (word, off) = ((at / 32) as usize, at % 32);
        let x = v << off;
        w[word] |= x as u32;
        if off + u64::from(bits) > 32 {
            w[word + 1] |= (x >> 32) as u32;
        }
    }
    w
}

/// Slot `j` of packed words (`None` out of bounds). All ones reads as `NONE`.
pub fn unpack_slot(words: &[u32], bits: u32, j: usize) -> Option<u32> {
    if bits == 0 || bits > 32 {
        return None;
    }
    let at = (j as u64).checked_mul(u64::from(bits))?;
    let word = usize::try_from(at / 32).ok()?;
    let off = at % 32;
    let lo = u64::from(*words.get(word)?);
    let hi = if off + u64::from(bits) > 32 { u64::from(*words.get(word + 1)?) } else { 0 };
    let mask = if bits >= 32 { u64::from(u32::MAX) } else { (1u64 << bits) - 1 };
    let v = (((hi << 32) | lo) >> off) & mask;
    Some(if v == mask { NONE } else { v as u32 })
}

mod sealed {
    pub trait Sealed {}
}

/// Plain-data types a section may be read as.
///
/// # Safety
/// Implementors are `repr(C)` (or primitive), valid for any bit pattern,
/// and have no padding.
pub unsafe trait Pod: Copy + 'static + sealed::Sealed {}

macro_rules! pod {
    ($($t:ty),*) => {$(
        impl sealed::Sealed for $t {}
        // SAFETY: primitives, arrays of them, or repr(C) structs of 4-byte
        // fields with no padding; any bit pattern is a value.
        unsafe impl Pod for $t {}
    )*};
}
pod!(u8, u32, u64, f32, [u64; 2], [u32; 2], [u32; 3], FaceNum, TermRec, PhraseRec, SituRec, CanonRec, BoostRec);

/// A slice of records as bytes, for the writer. Little-endian hosts only:
/// on others the bytes would not be the file format.
#[cfg(target_endian = "little")]
pub fn as_bytes<T: Pod>(v: &[T]) -> &[u8] {
    // SAFETY: T is plain data with no padding (Pod), so every byte is
    // initialised; the length is the slice's size in bytes.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Bytes as a slice of records: `None` if misaligned or not a whole number
/// of records.
pub(crate) fn cast<T: Pod>(b: &[u8]) -> Option<&[T]> {
    let size = std::mem::size_of::<T>();
    if size == 0 || !b.len().is_multiple_of(size) || !(b.as_ptr() as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return None;
    }
    // SAFETY: aligned and in bounds (checked above), and T is valid for any
    // bit pattern (Pod).
    Some(unsafe { std::slice::from_raw_parts(b.as_ptr() as *const T, b.len() / size) })
}

/// CRC-32 (IEEE 802.3, the zlib one) of `data`: the per-section checksum in
/// the directory.
pub fn crc32(data: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    }
    static TABLE: [u32; 256] = table();
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// One parsed directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionEntry {
    pub tag: Tag,
    pub flags: u32,
    pub offset: u64,
    pub len: u64,
    pub elem_size: u32,
    pub crc32: u32,
}

impl SectionEntry {
    /// The entry's 32 bytes.
    pub fn to_bytes(&self) -> [u8; DIR_ENTRY_LEN] {
        let mut b = [0u8; DIR_ENTRY_LEN];
        b[0..4].copy_from_slice(&self.tag);
        b[4..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..16].copy_from_slice(&self.offset.to_le_bytes());
        b[16..24].copy_from_slice(&self.len.to_le_bytes());
        b[24..28].copy_from_slice(&self.elem_size.to_le_bytes());
        b[28..32].copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    /// Parse 32 bytes (`None` if fewer).
    pub fn from_bytes(b: &[u8]) -> Option<SectionEntry> {
        let b: &[u8; DIR_ENTRY_LEN] = b.get(..DIR_ENTRY_LEN)?.try_into().ok()?;
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64_at = |o: usize| u64::from(u32_at(o)) | (u64::from(u32_at(o + 4)) << 32);
        Some(SectionEntry {
            tag: [b[0], b[1], b[2], b[3]],
            flags: u32_at(4),
            offset: u64_at(8),
            len: u64_at(16),
            elem_size: u32_at(24),
            crc32: u32_at(28),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_sizes_are_fixed() {
        assert_eq!(std::mem::size_of::<FaceNum>(), 116);
        assert_eq!(std::mem::size_of::<TermRec>(), 92);
        assert_eq!(std::mem::size_of::<PhraseRec>(), 104);
        assert_eq!(std::mem::size_of::<SituRec>(), 104);
        assert_eq!(std::mem::size_of::<CanonRec>(), 24);
        assert_eq!(std::mem::size_of::<BoostRec>(), 24);
    }

    #[test]
    fn varints_and_packing_round_trip() {
        let mut b = Vec::new();
        let vals = [0u64, 1, 127, 128, 300, 16_383, 16_384, u32::MAX as u64, u64::MAX];
        for v in vals {
            put_varint(&mut b, v);
        }
        let mut pos = 0;
        for v in vals {
            assert_eq!(get_varint(&b, &mut pos), Some(v));
        }
        assert_eq!(get_varint(&b, &mut pos), None, "end");
        assert_eq!(get_varint(&[0xff; 12], &mut 0), None, "too long");
        for bits in [1, 5, 17, 18, 31, 32] {
            let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
            let slots: Vec<u32> = (0..100u32).map(|i| if i % 7 == 3 { NONE } else { i.wrapping_mul(2_654_435_761) & mask & !1 }).collect();
            let w = pack_slots(&slots, bits);
            for (j, &s) in slots.iter().enumerate() {
                let want = if s == NONE || s == mask { NONE } else { s };
                assert_eq!(unpack_slot(&w, bits, j), Some(want), "bits {bits} slot {j}");
            }
            assert_eq!(unpack_slot(&w, bits, 10_000), None);
        }
        assert_eq!(packed_bits(0), 1);
        assert_eq!(packed_bits(81_314), 17);
        assert_eq!(packed_bits(131_071), 17, "131071 faces: ordinals up to 131070, none = 131071");
        assert_eq!(packed_bits(131_072), 18);
    }

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn directory_entry_round_trips() {
        let e = SectionEntry { tag: *b"FIDS", flags: 1, offset: 4096, len: 80, elem_size: 8, crc32: 0xdead_beef };
        assert_eq!(SectionEntry::from_bytes(&e.to_bytes()), Some(e));
        assert_eq!(SectionEntry::from_bytes(&[0; 31]), None);
    }

    #[test]
    fn cast_checks_alignment_and_length() {
        let v: Vec<u64> = vec![1, 2];
        let b = as_bytes(&v);
        assert_eq!(cast::<u64>(b), Some(&[1u64, 2][..]));
        assert!(cast::<u64>(&b[..15]).is_none());
        assert!(cast::<u32>(&b[1..13]).is_none());
    }
}
