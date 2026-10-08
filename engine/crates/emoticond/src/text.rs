//! Small text helpers the engine shares with the data compiler.

/// The face's distinct non-space chars, sorted: the near-duplicate test's input.
pub fn char_set(text: &str) -> Vec<char> {
    let mut v: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Jaccard similarity of two sorted, de-duplicated sets.
pub fn jaccard<T: Ord>(a: &[T], b: &[T]) -> f32 {
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

/// Damerau-Levenshtein distance, giving up (cap + 1) once it exceeds `cap`.
pub fn damerau(a: &[char], b: &[char], cap: usize) -> usize {
    if a.len().abs_diff(b.len()) > cap {
        return cap + 1;
    }
    let mut prev2: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![0usize; b.len() + 1];
        cur[0] = i;
        let mut best = cur[0];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if !prev2.is_empty()
                && i > 1
                && j > 1
                && a[i - 1] == b[j - 2]
                && a[i - 2] == b[j - 1]
            {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
            best = best.min(cur[j]);
        }
        if best > cap {
            return cap + 1;
        }
        prev2 = prev;
        prev = cur;
    }
    prev[b.len()]
}
