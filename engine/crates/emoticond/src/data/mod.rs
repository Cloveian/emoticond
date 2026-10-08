//! The data file (`.kmj`, format 3) over bytes: a memory map, an owned
//! buffer, or a `&'static` slice (docs/format.md).
//!
//! Opening checks the header, the section directory and the cross-section
//! lengths: O(sections), never O(data), so it costs well under a
//! millisecond whatever the file's size. Contents are not scanned. Every
//! accessor bounds-checks what it reads (spans, ordinals, ranges), and
//! strings are UTF-8-checked when read, so a damaged file gives missing or
//! wrong results, never a panic or a read outside the buffer.
//! [`DataFile::verify`] runs the full checksum pass on demand.

pub mod format;
pub mod manifest;

use crate::error::OpenError;
use crate::grammar::Grammar;
use crate::id::FaceId;
use format::*;
pub use manifest::Manifest;

/// The bytes of a data set, for [`Database::from_bytes`](crate::Database::from_bytes).
///
/// Zero-copy needs an 8-aligned buffer: a misaligned one is copied once.
/// For embedding, [`include_data!`](crate::include_data) gives an aligned
/// `&'static [u8]`.
#[derive(Debug)]
pub enum DataBytes {
    /// Borrowed for the life of the program (`include_bytes!`).
    Static(&'static [u8]),
    /// An owned buffer (a file read into memory, a `fetch()` result).
    Owned(Vec<u8>),
}

impl From<Vec<u8>> for DataBytes {
    fn from(v: Vec<u8>) -> DataBytes {
        DataBytes::Owned(v)
    }
}
impl From<Box<[u8]>> for DataBytes {
    fn from(v: Box<[u8]>) -> DataBytes {
        DataBytes::Owned(v.into_vec())
    }
}
impl From<&'static [u8]> for DataBytes {
    fn from(v: &'static [u8]) -> DataBytes {
        DataBytes::Static(v)
    }
}
impl<const N: usize> From<&'static [u8; N]> for DataBytes {
    fn from(v: &'static [u8; N]) -> DataBytes {
        DataBytes::Static(v)
    }
}

/// `include_bytes!` into an 8-aligned static, for
/// [`Database::from_bytes`](crate::Database::from_bytes) without a copy.
///
/// ```ignore
/// let db = emoticond::Database::from_bytes(emoticond::include_data!("core.kmj"), Default::default())?;
/// ```
#[macro_export]
macro_rules! include_data {
    ($path:expr) => {{
        #[repr(C, align(8))]
        struct __EmoticondAligned<B: ?Sized>(B);
        static __EMOTICOND_DATA: &__EmoticondAligned<[u8]> = &__EmoticondAligned(*include_bytes!($path));
        &__EMOTICOND_DATA.0
    }};
}

/// Where the bytes live. Every variant is 8-aligned.
enum Bytes {
    #[cfg(feature = "mmap")]
    Map(memmap2::Mmap),
    Static(&'static [u8]),
    Vec(Vec<u8>),
    /// A misaligned buffer copied into u64 words, and its length in bytes.
    Words(Vec<u64>, usize),
}

impl Bytes {
    fn get(&self) -> &[u8] {
        match self {
            #[cfg(feature = "mmap")]
            Bytes::Map(m) => m,
            Bytes::Static(s) => s,
            Bytes::Vec(v) => v,
            Bytes::Words(w, n) => {
                // SAFETY: u64s are plain bytes; `n` <= 8 * w.len() by construction.
                let all = unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 8) };
                all.get(..*n).unwrap_or_default()
            }
        }
    }

    fn aligned(b: &[u8]) -> bool {
        (b.as_ptr() as usize).is_multiple_of(8)
    }

    /// Copy into aligned words.
    fn copy(b: &[u8]) -> Bytes {
        let mut w = vec![0u64; b.len().div_ceil(8)];
        for (dst, src) in w.iter_mut().zip(b.chunks(8)) {
            let mut x = [0u8; 8];
            x[..src.len()].copy_from_slice(src);
            *dst = u64::from_ne_bytes(x);
        }
        Bytes::Words(w, b.len())
    }

    fn from_data(d: DataBytes) -> Bytes {
        match d {
            DataBytes::Static(s) if Bytes::aligned(s) => Bytes::Static(s),
            DataBytes::Static(s) => Bytes::copy(s),
            DataBytes::Owned(v) if Bytes::aligned(&v) => Bytes::Vec(v),
            DataBytes::Owned(v) => Bytes::copy(&v),
        }
    }
}

// Indices into `SECTIONS` (checked by a test).
const MANI: usize = 0;
const LICN: usize = 1;
const ATTR: usize = 2;
const GRAM: usize = 3;
const STRS: usize = 4;
const FNUM: usize = 5;
const FIDS: usize = 6;
const FTXT: usize = 7;
const FCHR: usize = 8;
const CHRS: usize = 9;
const BTXT: usize = 10;
const TERM: usize = 11;
const TKEY: usize = 12;
const TSRT: usize = 13;
const DNSE: usize = 14;
const ENGN: usize = 15;
const WKEY: usize = 16;
const WPST: usize = 17;
const POST: usize = 18;
const SLST: usize = 19;
const PHRS: usize = 20;
const PIDX: usize = 21;
const SITU: usize = 22;
const SMAT: usize = 23;
const CANO: usize = 24;
const BOST: usize = 25;
const FNQ2: usize = 26;
const FNQ1: usize = 27;
const FNCB: usize = 28;
const DNSP: usize = 29;
const ENGP: usize = 30;
const PSTV: usize = 31;
const ALIA: usize = 32;
const RETD: usize = 33;
const N_KNOWN: usize = 34;

/// Sections a file must have, as alternatives: one of each group (format
/// 3.1 adds the compact encodings as alternatives to 3.0's tables).
const MUST_HAVE: &[&[usize]] = &[
    &[MANI],
    &[LICN],
    &[ATTR],
    &[STRS],
    &[FNUM, FNQ2, FNQ1],
    &[FIDS],
    &[FTXT],
    &[BTXT],
    &[TERM],
    &[TKEY],
    &[TSRT],
    &[DNSE, DNSP],
    &[WKEY],
    &[WPST],
    &[POST, PSTV],
    &[SLST],
    &[PHRS],
    &[PIDX],
    &[SITU],
    &[SMAT],
    &[CANO],
];

/// How a face's numbers are stored.
#[derive(Clone, Copy)]
enum NumsAt {
    /// `FNUM`: f32 records.
    F32,
    /// `FNQ2` (2-byte codes) or `FNQ1` (1-byte codes), decoded through `FNCB`.
    Coded { wide: bool },
}

/// A view of every face's numbers, whatever the encoding (`FNUM`, or the
/// coded `FNQ2`/`FNQ1` of format 3.1). Records are decoded on access, so it
/// hands out values, not references.
#[derive(Clone, Copy)]
pub struct Nums<'a> {
    at: NumsAt,
    f32s: &'a [FaceNum],
    codes: &'a [u8],
    /// per coded field: its decode table
    book: [&'a [f32]; N_CODED],
    n: usize,
}

impl<'a> Nums<'a> {
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Face `i`'s numbers (`None` past the end).
    pub fn get(&self, i: usize) -> Option<FaceNum> {
        if i >= self.n {
            return None;
        }
        match self.at {
            NumsAt::F32 => self.f32s.get(i).copied(),
            NumsAt::Coded { wide } => {
                let stride = if wide { FACE_Q2_LEN } else { FACE_Q1_LEN };
                let rec = self.codes.get(i * stride..(i + 1) * stride)?;
                let mut v = [0f32; N_CODED];
                for (k, x) in v.iter_mut().enumerate() {
                    let c = if wide { usize::from(u16::from_le_bytes([rec[2 * k], rec[2 * k + 1]])) } else { usize::from(rec[k]) };
                    *x = self.book[k].get(c).copied().unwrap_or(0.0);
                }
                let len = u16::from_le_bytes([rec[stride - 2], rec[stride - 1]]);
                Some(FaceNum::from_coded(&v, u32::from(len)))
            }
        }
    }
    /// Face `i`'s numbers, or zeros past the end.
    pub fn at(&self, i: usize) -> FaceNum {
        self.get(i).unwrap_or_default()
    }
    /// Every face's numbers, in ordinal order.
    pub fn iter(&self) -> impl Iterator<Item = FaceNum> + 'a {
        let me = *self;
        (0..self.n).map(move |i| me.at(i))
    }
}

/// A term's neighbour list: (rank, ordinal) in rank order; `NONE` marks an
/// empty slot (a face not in this set), which keeps the ranks after it.
pub enum Neighbours<'a> {
    Plain(std::iter::Enumerate<std::slice::Iter<'a, u32>>),
    Packed { words: &'a [u32], bits: u32, at: usize, end: usize, rank: usize },
}

impl Iterator for Neighbours<'_> {
    type Item = (usize, u32);
    fn next(&mut self) -> Option<(usize, u32)> {
        match self {
            Neighbours::Plain(it) => it.next().map(|(r, &o)| (r, o)),
            Neighbours::Packed { words, bits, at, end, rank } => {
                if *at >= *end {
                    return None;
                }
                let o = unpack_slot(words, *bits, *at)?;
                let r = *rank;
                *at += 1;
                *rank += 1;
                Some((r, o))
            }
        }
    }
}

/// A tag word's postings: (ordinal, tier) in ordinal order.
pub enum Postings<'a> {
    Plain(std::slice::Iter<'a, u32>),
    /// `PSTV`: varints of `(ordinal delta << 2) | tier`
    Varint { bytes: &'a [u8], pos: usize, ord: u64 },
}

impl Iterator for Postings<'_> {
    type Item = (u32, u8);
    fn next(&mut self) -> Option<(u32, u8)> {
        match self {
            Postings::Plain(it) => it.next().map(|&p| (p >> 2, (p & 3) as u8)),
            Postings::Varint { bytes, pos, ord } => {
                let v = get_varint(bytes, pos)?;
                *ord = ord.saturating_add(v >> 2);
                Some((u32::try_from(*ord).unwrap_or(NONE), (v & 3) as u8))
            }
        }
    }
}

/// What a data file knows about a face id (format 3.1 `ALIA`/`RETD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdStatus {
    /// A face of this set.
    Live,
    /// Renamed: the id it became (which may or may not be in this set).
    Aliased(FaceId),
    /// Removed from the data on purpose, with a `RETIRED_*` reason.
    Retired(u32),
    /// Not known to this file (for a smaller set: a face of another set).
    Unknown,
}

const ZERO_TERM: TermRec = TermRec { p: [0.0; N_EMO], m: 0.0, n: 0, tier: 0, flags: 0 };

/// The parsed fixed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format_major: u16,
    pub format_minor: u16,
    pub flags: u32,
    pub header_len: u64,
    pub n_sections: u32,
    /// SHA-256 of every byte after the fixed header (directory and sections).
    pub content_hash: [u8; 32],
}

/// A phrase of the generated search lexicon, read from the file.
pub(crate) struct PhraseView<'a> {
    pub key: &'a str,
    pub p: [f32; N_EMO],
    pub level: Option<u8>,
    pub pair: Option<bool>,
    pub words: Vec<&'a str>,
    pub cute: bool,
    pub lenny: bool,
    pub lewd: bool,
    /// beats a vocab term of the same spelling however well the data knows it
    pub over: bool,
    /// expected share of multi-figure faces (user aliases of vocab terms only)
    pub m: Option<f32>,
    /// from the user's overlay
    pub user: bool,
}

/// A hand-written situation, read from the file.
pub(crate) struct SituationView<'a> {
    pub name: &'a str,
    pub p: [f32; N_EMO],
    pub pair: Option<bool>,
    /// Each spelling as tokens.
    pub matches: Vec<Vec<&'a str>>,
    pub words: Vec<&'a str>,
}

fn opt_bool(v: u32) -> Option<bool> {
    match v {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// An open data file: the bytes, the checked section directory, and the
/// small tables parsed at open (manifest, grammar).
pub struct DataFile {
    bytes: Bytes,
    header: Header,
    /// (offset, len) of each known section; (0, 0) when absent.
    sec: [(usize, usize); N_KNOWN],
    directory: Vec<SectionEntry>,
    manifest: Manifest,
    grammar: Grammar,
    pub(crate) n_faces: usize,
    pub(crate) n_terms: usize,
    pub(crate) dense_k: usize,
    /// which known sections the file has
    seen: [bool; N_KNOWN],
    nums_at: NumsAt,
    /// `FNCB`: per coded field, the range of its table in the section
    book: [(usize, usize); N_CODED],
    /// `DNSP`/`ENGP`: slot width
    dnsp_bits: u32,
    engp_bits: u32,
    /// coded face numbers (`FNQ2`/`FNQ1`), decoded on first use: scoring
    /// reads every face's numbers on every query, and table lookups there
    /// cost more than the copy (the same memory as an `FNUM` section)
    decoded: std::sync::OnceLock<Vec<FaceNum>>,
}

impl std::fmt::Debug for DataFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataFile")
            .field("len", &self.bytes.get().len())
            .field("format", &(self.header.format_major, self.header.format_minor))
            .field("faces", &self.n_faces)
            .finish()
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).and_then(|x| x.try_into().ok()).map_or(0, u16::from_le_bytes)
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).and_then(|x| x.try_into().ok()).map_or(0, u32::from_le_bytes)
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).and_then(|x| x.try_into().ok()).map_or(0, u64::from_le_bytes)
}

fn corrupt(i: usize) -> OpenError {
    OpenError::Corrupt { section: SECTIONS.get(i).map_or("directory", |s| s.1) }
}

/// Parse and check the fixed header (`len` = the file's size).
pub fn read_header(b: &[u8]) -> Result<Header, OpenError> {
    if b.len() < MAGIC.len() || b[..MAGIC.len()] != MAGIC {
        let found = if b.starts_with(b"EMOIDX") {
            format!("an {} engine index (the old format; compile a .kmj instead)", String::from_utf8_lossy(&b[..b.len().min(8)]))
        } else {
            "not a kaomoji data file".to_string()
        };
        return Err(OpenError::Incompatible { found, supported: "kaomoji data format 3.x" });
    }
    if b.len() < HEADER_LEN {
        return Err(OpenError::Corrupt { section: "header" });
    }
    let h = Header {
        format_major: u16_at(b, 8),
        format_minor: u16_at(b, 10),
        flags: u32_at(b, 12),
        header_len: u64_at(b, 16),
        n_sections: u32_at(b, 24),
        content_hash: b.get(32..64).and_then(|x| x.try_into().ok()).unwrap_or([0; 32]),
    };
    if h.format_major != FORMAT_MAJOR {
        return Err(OpenError::Incompatible {
            found: format!("kaomoji data format {}.{}", h.format_major, h.format_minor),
            supported: "kaomoji data format 3.x",
        });
    }
    let n = h.n_sections as usize;
    let want = (HEADER_LEN as u64).checked_add(n as u64 * DIR_ENTRY_LEN as u64);
    if n > MAX_SECTIONS || want != Some(h.header_len) || h.header_len > b.len() as u64 {
        return Err(OpenError::Corrupt { section: "directory" });
    }
    Ok(h)
}

impl DataFile {
    /// Open a data file by path: memory-mapped with the `mmap` feature,
    /// read into memory without it.
    ///
    /// Data files must be replaced by rename, never rewritten in place: a
    /// mapped file that changes under a reader can crash it (SIGBUS).
    #[cfg(feature = "fs")]
    pub fn open(path: &std::path::Path) -> Result<DataFile, OpenError> {
        #[cfg(feature = "mmap")]
        {
            let file = std::fs::File::open(path)?;
            // SAFETY: data files are only ever replaced by rename (documented
            // above and in docs/format.md), so the mapping does not change.
            let map = unsafe { memmap2::Mmap::map(&file) }?;
            DataFile::new(Bytes::Map(map))
        }
        #[cfg(not(feature = "mmap"))]
        {
            DataFile::from_bytes(DataBytes::Owned(std::fs::read(path)?))
        }
    }

    /// Open from bytes in memory (copied once if not 8-aligned).
    pub fn from_bytes(bytes: impl Into<DataBytes>) -> Result<DataFile, OpenError> {
        DataFile::new(Bytes::from_data(bytes.into()))
    }

    fn new(bytes: Bytes) -> Result<DataFile, OpenError> {
        if cfg!(target_endian = "big") {
            return Err(OpenError::BigEndian);
        }
        let b = bytes.get();
        let header = read_header(b)?;
        let mut sec = [(0usize, 0usize); N_KNOWN];
        let mut seen = [false; N_KNOWN];
        let mut grams = Vec::new();
        let mut directory = Vec::with_capacity(header.n_sections as usize);
        for k in 0..header.n_sections as usize {
            let o = HEADER_LEN + k * DIR_ENTRY_LEN;
            let e = b.get(o..).and_then(SectionEntry::from_bytes).ok_or(OpenError::Corrupt { section: "directory" })?;
            directory.push(e);
            let Some(i) = SECTIONS.iter().position(|s| s.0 == e.tag) else {
                if e.flags & SECTION_REQUIRED != 0 {
                    return Err(OpenError::Incompatible {
                        found: format!(
                            "kaomoji data format {}.{} with a required section {:?} this library does not know",
                            header.format_major,
                            header.format_minor,
                            String::from_utf8_lossy(&e.tag)
                        ),
                        supported: "kaomoji data format 3.x",
                    });
                }
                continue;
            };
            let end = e.offset.checked_add(e.len);
            if e.elem_size as usize != SECTIONS[i].2
                || e.offset % 8 != 0
                || e.offset < header.header_len
                || end.is_none_or(|end| end > b.len() as u64)
                || e.len % u64::from(e.elem_size.max(1)) != 0
            {
                return Err(corrupt(i));
            }
            let span = (e.offset as usize, e.len as usize);
            if i == GRAM {
                grams.push(span);
            } else if std::mem::replace(&mut seen[i], true) {
                return Err(corrupt(i));
            }
            sec[i] = span;
        }
        if let Some(g) = MUST_HAVE.iter().find(|g| !g.iter().any(|&i| seen[i])) {
            return Err(corrupt(g[0]));
        }
        // at most one encoding of a table
        for g in [&[FNUM, FNQ2, FNQ1][..], &[DNSE, DNSP], &[ENGN, ENGP], &[POST, PSTV]] {
            if g.iter().filter(|&&i| seen[i]).count() > 1 {
                return Err(corrupt(g[0]));
            }
        }
        // the char sets are both there or both derived from the text (3.1)
        if seen[FCHR] != seen[CHRS] {
            return Err(corrupt(if seen[FCHR] { CHRS } else { FCHR }));
        }
        let raw = |(o, l): (usize, usize)| b.get(o..o + l).unwrap_or_default();
        let manifest = Manifest::parse(raw(sec[MANI])).filter(|m| m.emotions.len() == N_EMO).ok_or(corrupt(MANI))?;
        let mut grammar: Option<Grammar> = None;
        for &g in &grams {
            let one = Grammar::parse(raw(g)).ok_or(corrupt(GRAM))?;
            match grammar.as_mut() {
                None => grammar = Some(one),
                Some(all) => all.merge(&one),
            }
        }
        let grammar = grammar.ok_or(corrupt(GRAM))?;
        for i in [LICN, ATTR] {
            std::str::from_utf8(raw(sec[i])).map_err(|_| corrupt(i))?;
        }

        // cross-section lengths
        let count = |i: usize| sec[i].1 / SECTIONS[i].2;
        let n = count(FIDS);
        let nt = count(TERM);
        let k = manifest.dense_k;
        let lists = nt.checked_mul(k).ok_or(corrupt(DNSE))?;
        let nums_at = if seen[FNUM] { NumsAt::F32 } else { NumsAt::Coded { wide: seen[FNQ2] } };
        let n_nums = match nums_at {
            NumsAt::F32 => count(FNUM),
            NumsAt::Coded { wide: true } => count(FNQ2),
            NumsAt::Coded { wide: false } => count(FNQ1),
        };
        // FNCB: N_CODED + 1 table starts, then the tables (f32)
        let mut book = [(0usize, 0usize); N_CODED];
        if let NumsAt::Coded { .. } = nums_at {
            let cb: &[u32] = cast(raw(sec[FNCB])).unwrap_or_default();
            if cb.len() < N_CODED + 1 {
                return Err(corrupt(FNCB));
            }
            for (f, slot) in book.iter_mut().enumerate() {
                let (a, b) = (cb[f] as usize, cb[f + 1] as usize);
                if a > b || b > cb.len() - (N_CODED + 1) {
                    return Err(corrupt(FNCB));
                }
                *slot = (N_CODED + 1 + a, b - a);
            }
        }
        // DNSP/ENGP: bits, n_terms, n_terms + 1 starts, then the slots
        let packed = |i: usize| -> Result<u32, OpenError> {
            let w: &[u32] = cast(raw(sec[i])).unwrap_or_default();
            let ok = w.len() > PACKED_HEAD + nt && (1..=32).contains(&w[0]) && w[1] as usize == nt;
            if !ok {
                return Err(corrupt(i));
            }
            Ok(w[0])
        };
        let dnsp_bits = if seen[DNSP] { packed(DNSP)? } else { 0 };
        let engp_bits = if seen[ENGP] { packed(ENGP)? } else { 0 };
        let checks: [(usize, bool); 13] = [
            (FIDS, manifest.n_faces == n && n < u32::MAX as usize >> 2),
            (FNUM, n_nums == n),
            (FTXT, count(FTXT) == n),
            (FCHR, count(FCHR) == n || !seen[FCHR]),
            (BTXT, count(BTXT) == n),
            (TERM, manifest.n_terms == nt),
            (TKEY, count(TKEY) == nt),
            (TSRT, count(TSRT) <= nt),
            (DNSE, count(DNSE) == lists || seen[DNSP]),
            (ENGN, count(ENGN) == lists || sec[ENGN].1 == 0),
            (WPST, count(WPST) == count(WKEY)),
            (PHRS, count(PHRS) == manifest.n_phrases),
            (SITU, count(SITU) == manifest.n_situations),
        ];
        if let Some(&(i, _)) = checks.iter().find(|c| !c.1) {
            return Err(corrupt(i));
        }
        if count(CANO) != manifest.n_canonical {
            return Err(corrupt(CANO));
        }
        Ok(DataFile {
            bytes,
            header,
            sec,
            directory,
            manifest,
            grammar,
            n_faces: n,
            n_terms: nt,
            dense_k: k,
            seen,
            nums_at,
            book,
            dnsp_bits,
            engp_bits,
            decoded: std::sync::OnceLock::new(),
        })
    }

    /// Check every section's CRC-32 against the directory: a full read of
    /// the file (O(data)), for `doctor`-style checks, never done at open.
    /// Returns the tag of the first damaged section.
    pub fn verify(&self) -> Result<(), String> {
        let b = self.bytes.get();
        for e in &self.directory {
            let data = b.get(e.offset as usize..(e.offset + e.len) as usize).unwrap_or_default();
            if crc32(data) != e.crc32 {
                return Err(String::from_utf8_lossy(&e.tag).into_owned());
            }
        }
        Ok(())
    }

    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    /// The section directory, in file order.
    pub fn directory(&self) -> &[SectionEntry] {
        &self.directory
    }
    /// The size of the file in bytes.
    pub fn len(&self) -> usize {
        self.bytes.get().len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.get().is_empty()
    }
    /// The parser's word lists: the union of every `GRAM` section.
    pub(crate) fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    fn raw(&self, s: usize) -> &[u8] {
        let (o, l) = self.sec[s];
        self.bytes.get().get(o..o + l).unwrap_or_default()
    }
    fn slice<T: Pod>(&self, s: usize) -> &[T] {
        cast(self.raw(s)).unwrap_or_default()
    }
    fn text_section(&self, s: usize) -> &str {
        std::str::from_utf8(self.raw(s)).unwrap_or_default()
    }
    /// The data licence text (`LICN`).
    pub fn licence(&self) -> &str {
        self.text_section(LICN)
    }
    /// Attribution and notices (`ATTR`).
    pub fn attribution(&self) -> &str {
        self.text_section(ATTR)
    }

    /// A string from the heap; "" if the span is out of bounds or not UTF-8.
    fn str_at(&self, [a, b]: [u32; 2]) -> &str {
        let raw = self.slice::<u8>(STRS).get(a as usize..b as usize).unwrap_or_default();
        std::str::from_utf8(raw).unwrap_or_default()
    }
    fn span(&self, s: usize, i: usize) -> [u32; 2] {
        self.slice::<[u32; 2]>(s).get(i).copied().unwrap_or([0, 0])
    }
    /// The strings an `SLST` range names.
    fn str_list(&self, [a, b]: [u32; 2]) -> Vec<&str> {
        let l = self.slice::<[u32; 2]>(SLST).get(a as usize..b as usize).unwrap_or_default();
        l.iter().map(|&s| self.str_at(s)).collect()
    }

    // ---- faces --------------------------------------------------------

    /// Every face's numbers. Coded numbers (`FNQ2`/`FNQ1`) are decoded
    /// once, on the first call, and kept.
    pub fn nums(&self) -> Nums<'_> {
        let f32s: &[FaceNum] = match self.nums_at {
            NumsAt::F32 => self.slice(FNUM),
            NumsAt::Coded { .. } => self.decoded.get_or_init(|| self.coded_nums().iter().collect()),
        };
        Nums { at: NumsAt::F32, f32s, codes: &[], book: [&[]; N_CODED], n: self.n_faces }
    }
    /// The coded numbers, decoded per access (no cache).
    fn coded_nums(&self) -> Nums<'_> {
        let mut book: [&[f32]; N_CODED] = [&[]; N_CODED];
        let all = self.slice::<f32>(FNCB);
        for (b, &(a, l)) in book.iter_mut().zip(&self.book) {
            *b = all.get(a..a + l).unwrap_or_default();
        }
        let codes = match self.nums_at {
            NumsAt::Coded { wide: true } => self.raw(FNQ2),
            NumsAt::Coded { wide: false } => self.raw(FNQ1),
            NumsAt::F32 => &[],
        };
        Nums { at: self.nums_at, f32s: &[], codes, book, n: self.n_faces }
    }
    /// Whether the face numbers are stored coded (`FNQ2`/`FNQ1`).
    pub fn nums_encoding(&self) -> &'static str {
        match self.nums_at {
            NumsAt::F32 => "f32",
            NumsAt::Coded { wide: true } => "q16",
            NumsAt::Coded { wide: false } => "q8",
        }
    }
    /// The face's stable id.
    pub fn fid(&self, i: usize) -> FaceId {
        let v = self.slice::<u64>(FIDS).get(i).copied().unwrap_or(0);
        FaceId::from_u64(v).unwrap_or_else(|| FaceId::of_text(self.text(i)))
    }
    /// The face with this id (`FIDS` is sorted), following aliases
    /// (`ALIA`): an id renamed in a later release finds the face it became.
    pub fn by_fid(&self, id: FaceId) -> Option<u32> {
        let v = self.slice::<u64>(FIDS);
        let mut id = id.as_u64();
        // aliases are resolved to their end by the compiler; the bound only
        // guards against a damaged file's cycles
        for _ in 0..8 {
            if let Ok(i) = v.binary_search(&id) {
                return Some(i as u32);
            }
            id = self.alias_of(id)?;
        }
        None
    }
    fn alias_of(&self, id: u64) -> Option<u64> {
        let a = self.slice::<[u64; 2]>(ALIA);
        a.binary_search_by_key(&id, |r| r[0]).ok().and_then(|i| a.get(i)).map(|r| r[1])
    }
    /// What this file knows about an id: a face of this set, renamed,
    /// retired, or unknown.
    pub fn id_status(&self, id: FaceId) -> IdStatus {
        if self.slice::<u64>(FIDS).binary_search(&id.as_u64()).is_ok() {
            return IdStatus::Live;
        }
        if let Some(new) = self.alias_of(id.as_u64()).and_then(FaceId::from_u64) {
            return IdStatus::Aliased(new);
        }
        let r = self.slice::<[u64; 2]>(RETD);
        match r.binary_search_by_key(&id.as_u64(), |x| x[0]) {
            Ok(i) => IdStatus::Retired(r.get(i).map_or(RETIRED_OTHER, |x| x[1] as u32)),
            Err(_) => IdStatus::Unknown,
        }
    }
    /// Number of aliases and retired ids.
    pub fn n_aliases(&self) -> (usize, usize) {
        (self.slice::<[u64; 2]>(ALIA).len(), self.slice::<[u64; 2]>(RETD).len())
    }
    /// Face ordinals in id order (which is ordinal order).
    pub fn ordinals_by_fid(&self) -> impl Iterator<Item = u32> + '_ {
        0..self.n_faces as u32
    }
    /// Emotion names, in the order of `FaceNum::r`.
    pub fn emotions(&self) -> Vec<String> {
        self.manifest.emotions.clone()
    }
    pub fn text(&self, i: usize) -> &str {
        self.str_at(self.span(FTXT, i))
    }
    /// The face's distinct non-space chars as code points, sorted
    /// (text::char_set): enough for text::jaccard, which only compares.
    /// Stored (`FCHR`/`CHRS`), or derived from the text when the file
    /// leaves them out (format 3.1).
    pub fn chars(&self, i: usize) -> std::borrow::Cow<'_, [u32]> {
        if !self.seen[FCHR] && i < self.n_faces {
            return std::borrow::Cow::Owned(crate::text::char_set(self.text(i)).into_iter().map(|c| c as u32).collect());
        }
        let [a, b] = self.span(FCHR, i);
        std::borrow::Cow::Borrowed(self.slice::<u32>(CHRS).get(a as usize..b as usize).unwrap_or_default())
    }
    /// Whether `chars` is stored (else derived from the text).
    pub fn has_chars(&self) -> bool {
        self.seen[FCHR]
    }
    /// The face with exactly this text.
    pub fn find_text(&self, text: &str) -> Option<u32> {
        let v = self.slice::<u32>(BTXT);
        let end = v.partition_point(|&i| self.text(i as usize) <= text);
        let i = *v.get(end.checked_sub(1)?)?;
        ((i as usize) < self.n_faces && self.text(i as usize) == text).then_some(i)
    }

    // ---- vocab --------------------------------------------------------

    pub fn term(&self, i: usize) -> &TermRec {
        self.slice::<TermRec>(TERM).get(i).unwrap_or(&ZERO_TERM)
    }
    fn term_key(&self, t: u32) -> &str {
        self.str_at(self.span(TKEY, t as usize))
    }
    /// Vocab keys in sorted order, each with its term.
    pub fn keys(&self) -> impl Iterator<Item = (&str, usize)> + Clone {
        self.slice::<u32>(TSRT).iter().map(|&t| (self.term_key(t), t as usize))
    }
    /// Vocab keys from the first that is >= p, in sorted order.
    pub fn keys_from(&self, p: &str) -> impl Iterator<Item = (&str, usize)> {
        let sorted = self.slice::<u32>(TSRT);
        let start = sorted.partition_point(|&t| self.term_key(t) < p);
        sorted.get(start..).unwrap_or_default().iter().map(|&t| (self.term_key(t), t as usize))
    }
    /// The term a vocab key names.
    pub fn term_index(&self, k: &str) -> Option<usize> {
        let sorted = self.slice::<u32>(TSRT);
        let i = sorted.binary_search_by(|&t| self.term_key(t).cmp(k)).ok()?;
        sorted.get(i).map(|&t| t as usize)
    }
    fn list(&self, s: usize, p: usize, bits: u32, t: usize) -> Neighbours<'_> {
        let k = self.dense_k;
        if bits != 0 {
            let w = self.slice::<u32>(p);
            let starts = w.get(PACKED_HEAD..).unwrap_or_default();
            let (a, b) = match (starts.get(t), starts.get(t + 1)) {
                (Some(&a), Some(&b)) if a <= b => (a as usize, (b as usize).min((a as usize).saturating_add(k))),
                _ => (0, 0),
            };
            let words = w.get(PACKED_HEAD + self.n_terms + 1..).unwrap_or_default();
            return Neighbours::Packed { words, bits, at: a, end: b, rank: 0 };
        }
        let l = match t.checked_mul(k) {
            Some(a) => self.slice::<u32>(s).get(a..a.saturating_add(k)).unwrap_or_default(),
            None => &[],
        };
        Neighbours::Plain(l.iter().enumerate())
    }
    /// The term's e5 neighbour list: (rank, ordinal), `NONE` = none.
    pub fn dense(&self, t: usize) -> Neighbours<'_> {
        self.list(DNSE, DNSP, self.dnsp_bits, t)
    }
    /// The term's affect-engine neighbour list (empty when the set has none).
    pub fn engine(&self, t: usize) -> Neighbours<'_> {
        self.list(ENGN, ENGP, self.engp_bits, t)
    }
    /// Which encodings this file uses: (face numbers, neighbour lists,
    /// postings, char sets), for `info`-style output.
    pub fn encodings(&self) -> String {
        format!(
            "nums={},lists={},postings={},chars={}",
            self.nums_encoding(),
            if self.dnsp_bits != 0 { "packed" } else { "u32" },
            if self.seen[PSTV] { "varint" } else { "u32" },
            if self.has_chars() { "stored" } else { "derived" },
        )
    }

    /// Faces carrying a tag word, each with its tier (0 best .. 2).
    pub fn postings(&self, w: &str) -> Postings<'_> {
        let keys = self.slice::<[u32; 2]>(WKEY);
        let range = keys
            .binary_search_by(|&s| self.str_at(s).cmp(w))
            .ok()
            .and_then(|i| self.slice::<[u32; 2]>(WPST).get(i).copied())
            .unwrap_or([0, 0]);
        if self.seen[PSTV] {
            let bytes = self.raw(PSTV).get(range[0] as usize..range[1] as usize).unwrap_or_default();
            return Postings::Varint { bytes, pos: 0, ord: 0 };
        }
        let p = self.slice::<u32>(POST).get(range[0] as usize..range[1] as usize).unwrap_or_default();
        Postings::Plain(p.iter())
    }

    // ---- phrases, situations, canonical picks, boosts ----------------

    pub(crate) fn n_phrases(&self) -> usize {
        self.slice::<PhraseRec>(PHRS).len()
    }
    pub(crate) fn phrase(&self, i: usize) -> Option<PhraseView<'_>> {
        let r = self.slice::<PhraseRec>(PHRS).get(i)?;
        Some(PhraseView {
            key: self.str_at(r.key),
            p: r.p,
            level: (r.level != UNSET).then_some(r.level as u8),
            pair: opt_bool(r.pair),
            words: self.str_list(r.words),
            cute: r.flags & PHRASE_CUTE != 0,
            lenny: r.flags & PHRASE_LENNY != 0,
            lewd: r.flags & PHRASE_LEWD != 0,
            over: r.flags & PHRASE_OVER != 0,
            m: None,
            user: false,
        })
    }
    fn pidx(&self) -> &[[u32; 3]] {
        self.slice(PIDX)
    }
    /// The phrase a spelling (tokens joined by spaces) names.
    pub(crate) fn phrase_lookup(&self, s: &str) -> Option<usize> {
        let v = self.pidx();
        let i = v.binary_search_by(|r| self.str_at([r[0], r[1]]).cmp(s)).ok()?;
        v.get(i).map(|r| r[2] as usize)
    }
    /// Every phrase spelling, sorted.
    pub(crate) fn phrase_spellings(&self) -> impl Iterator<Item = &str> {
        self.pidx().iter().map(|r| self.str_at([r[0], r[1]]))
    }
    pub(crate) fn phrase_max(&self) -> usize {
        self.manifest.phrase_max
    }

    pub(crate) fn n_situations(&self) -> usize {
        self.slice::<SituRec>(SITU).len()
    }
    /// The situations, in name order.
    pub(crate) fn situations(&self) -> impl Iterator<Item = SituationView<'_>> {
        self.slice::<SituRec>(SITU).iter().map(|r| {
            let m = self.slice::<[u32; 2]>(SMAT).get(r.matches[0] as usize..r.matches[1] as usize).unwrap_or_default();
            SituationView {
                name: self.str_at(r.name),
                p: r.p,
                pair: opt_bool(r.pair),
                matches: m.iter().map(|&x| self.str_list(x)).collect(),
                words: self.str_list(r.words),
            }
        })
    }

    fn cano(&self) -> &[CanonRec] {
        self.slice(CANO)
    }
    fn canon_faces(&self, r: &CanonRec) -> Vec<(u32, usize)> {
        r.faces.iter().take(r.n as usize).copied().filter(|&f| (f as usize) < self.n_faces).enumerate().map(|(rank, f)| (f, rank)).collect()
    }
    /// A concept's canonical faces, best first: (ordinal, rank).
    pub(crate) fn canonical(&self, term: &str) -> Option<Vec<(u32, usize)>> {
        let v = self.cano();
        let i = v.binary_search_by(|r| self.str_at(r.term).cmp(term)).ok()?;
        v.get(i).map(|r| self.canon_faces(r))
    }
    pub(crate) fn has_canonical(&self, term: &str) -> bool {
        self.cano().binary_search_by(|r| self.str_at(r.term).cmp(term)).is_ok()
    }
    /// Every concept with canonical faces, in term order.
    pub(crate) fn canonical_all(&self) -> impl Iterator<Item = (&str, Vec<(u32, usize)>)> {
        self.cano().iter().map(|r| (self.str_at(r.term), self.canon_faces(r)))
    }
    pub(crate) fn n_canonical(&self) -> usize {
        self.cano().len()
    }

    fn bost(&self) -> &[BoostRec] {
        self.slice(BOST)
    }
    /// A term's curated boosts, by face id.
    pub(crate) fn boosts(&self, term: &str) -> Vec<(FaceId, f32)> {
        let v = self.bost();
        let lo = v.partition_point(|r| self.str_at(r.term) < term);
        let hi = v.partition_point(|r| self.str_at(r.term) <= term);
        v.get(lo..hi)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| Some((FaceId::from_u64(u64::from(r.id[0]) | (u64::from(r.id[1]) << 32))?, r.boost)))
            .collect()
    }
    pub(crate) fn has_boosts(&self, term: &str) -> bool {
        self.bost().binary_search_by(|r| self.str_at(r.term).cmp(term)).is_ok()
    }
    /// Terms with boosts.
    pub(crate) fn n_boost_terms(&self) -> usize {
        let v = self.bost();
        let mut n = 0;
        let mut last = None;
        for r in v {
            let t = self.str_at(r.term);
            if last != Some(t) {
                n += 1;
                last = Some(t);
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_indices_match_the_table() {
        let want = [
            (MANI, "MANI"),
            (LICN, "LICN"),
            (ATTR, "ATTR"),
            (GRAM, "GRAM"),
            (STRS, "STRS"),
            (FNUM, "FNUM"),
            (FIDS, "FIDS"),
            (FTXT, "FTXT"),
            (FCHR, "FCHR"),
            (CHRS, "CHRS"),
            (BTXT, "BTXT"),
            (TERM, "TERM"),
            (TKEY, "TKEY"),
            (TSRT, "TSRT"),
            (DNSE, "DNSE"),
            (ENGN, "ENGN"),
            (WKEY, "WKEY"),
            (WPST, "WPST"),
            (POST, "POST"),
            (SLST, "SLST"),
            (PHRS, "PHRS"),
            (PIDX, "PIDX"),
            (SITU, "SITU"),
            (SMAT, "SMAT"),
            (CANO, "CANO"),
            (BOST, "BOST"),
            (FNQ2, "FNQ2"),
            (FNQ1, "FNQ1"),
            (FNCB, "FNCB"),
            (DNSP, "DNSP"),
            (ENGP, "ENGP"),
            (PSTV, "PSTV"),
            (ALIA, "ALIA"),
            (RETD, "RETD"),
        ];
        assert_eq!(SECTIONS.len(), N_KNOWN);
        for (i, name) in want {
            assert_eq!(SECTIONS[i].1, name);
            assert_eq!(&SECTIONS[i].0, name.as_bytes());
        }
    }

    #[test]
    fn bad_headers_are_errors() {
        assert!(matches!(DataFile::from_bytes(Vec::new()), Err(OpenError::Incompatible { .. })));
        assert!(matches!(DataFile::from_bytes(b"EMOIDX03".to_vec()), Err(OpenError::Incompatible { .. })));
        let mut h = MAGIC.to_vec();
        assert!(matches!(DataFile::from_bytes(h.clone()), Err(OpenError::Corrupt { section: "header" })));
        h.resize(HEADER_LEN, 0);
        h[8] = 4;
        assert!(matches!(DataFile::from_bytes(h.clone()), Err(OpenError::Incompatible { .. })), "format 4");
        h[8] = 3;
        assert!(matches!(DataFile::from_bytes(h.clone()), Err(OpenError::Corrupt { section: "directory" })), "header_len 0");
        h[16] = 64;
        assert!(matches!(DataFile::from_bytes(h), Err(OpenError::Corrupt { section: "MANI" })), "no sections");
    }

    #[test]
    fn misaligned_bytes_are_copied() {
        let v: Vec<u64> = vec![0x0102_0304_0506_0708, 9];
        let b = format::as_bytes(&v);
        let c = Bytes::from_data(DataBytes::Owned(b[1..].to_vec()));
        assert_eq!(c.get(), &b[1..]);
        assert!(Bytes::aligned(c.get()));
    }
}
