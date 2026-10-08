//! Personal popularity (docs/options.md §4.1).
//!
//! The library never reads usage from disk and has no clock. A front-end
//! keeps a [`UsageState`] (persisted by `emoticond-state`), updates it with
//! [`record`] and [`decay`], passing the current time, and hands
//! [`to_map`]'s [`UsageMap`] to each query as `SearchOptions::usage`.
//!
//! Times are unix milliseconds throughout. Weights decay exponentially with
//! the state's half-life; a pick adds 1.0 to the face's global weight and,
//! when the pick has a concept (`term_key`), to that concept's weight too.

use crate::id::FaceId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Milliseconds in a day.
const DAY_MS: f64 = 86_400_000.0;
/// Weights below this are dropped by [`decay`].
const FORGET_BELOW: f32 = 1e-3;

/// What the search reads: already-decayed weights in 0..=1, per face and
/// per concept. A query's concept is its `term_key` (the parsed vocab term,
/// phrase key or situation), so `idk`, `i dunno` and `idk lol` share history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageMap {
    /// How much you use each face at all.
    pub global: BTreeMap<FaceId, f32>,
    /// What you picked for each concept. Counts about 4× `global` in ranking.
    pub by_term: BTreeMap<String, BTreeMap<FaceId, f32>>,
}

impl UsageMap {
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.by_term.is_empty()
    }

    /// The face's usage for a concept, combined: (global + 4 × by_term) / 5,
    /// in 0..=1. Values outside 0..=1 in the map are clamped.
    pub fn weight(&self, id: FaceId, term_key: Option<&str>) -> f32 {
        let g = self.global.get(&id).copied().unwrap_or(0.0);
        let t = term_key.and_then(|k| self.by_term.get(k)).and_then(|m| m.get(&id)).copied().unwrap_or(0.0);
        let clamp = |x: f32| if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        (clamp(g) + 4.0 * clamp(t)) / 5.0
    }
}

/// One pick: the face chosen, and the concept it was chosen for (if the
/// query had one and the user lets terms be remembered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub id: FaceId,
    pub term_key: Option<String>,
}

impl Pick {
    pub fn new(id: FaceId, term_key: Option<String>) -> Pick {
        Pick { id, term_key }
    }

    /// The pick of `hit` from `result` (its id and the result's `term_key`).
    pub fn from_hit(result: &crate::SearchResult, hit: &crate::Hit) -> Pick {
        Pick { id: hit.id, term_key: result.term_key.clone() }
    }
}

/// The persisted form: raw decayed pick weights as of `updated`.
///
/// Holds no raw query text and no per-pick timestamps (options.md §4.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct UsageState {
    /// Schema version (1).
    pub v: u16,
    /// Unix ms the weights are current as of.
    pub updated: u64,
    /// Half-life of a pick's weight, in days (options.md: 1..=3650, default 30).
    pub half_life_days: f32,
    pub global: BTreeMap<FaceId, f32>,
    pub by_term: BTreeMap<String, BTreeMap<FaceId, f32>>,
}

impl Default for UsageState {
    fn default() -> UsageState {
        UsageState { v: 1, updated: 0, half_life_days: 30.0, global: BTreeMap::new(), by_term: BTreeMap::new() }
    }
}

impl UsageState {
    pub fn new(half_life_days: f32) -> UsageState {
        UsageState { half_life_days: clamp_half_life(half_life_days), ..UsageState::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.by_term.is_empty()
    }

    /// (term, face) pairs held, plus global entries.
    pub fn len(&self) -> usize {
        self.global.len() + self.by_term.values().map(|m| m.len()).sum::<usize>()
    }

    /// Forget everything ("Clear history").
    pub fn clear(&mut self) {
        self.global.clear();
        self.by_term.clear();
    }
}

fn clamp_half_life(d: f32) -> f32 {
    if d.is_nan() {
        30.0
    } else {
        d.clamp(1.0, 3650.0)
    }
}

/// Decay every weight to `now` with `half_life_days` (which also becomes the
/// state's half-life), dropping weights that have faded to nothing. A `now`
/// earlier than `state.updated` (a clock that went back) decays nothing.
pub fn decay(state: &mut UsageState, now: u64, half_life_days: f32) {
    state.half_life_days = clamp_half_life(half_life_days);
    if now <= state.updated {
        return;
    }
    let days = (now - state.updated) as f64 / DAY_MS;
    let f = 0.5f64.powf(days / f64::from(state.half_life_days)) as f32;
    state.global.retain(|_, w| {
        *w *= f;
        *w >= FORGET_BELOW
    });
    for m in state.by_term.values_mut() {
        m.retain(|_, w| {
            *w *= f;
            *w >= FORGET_BELOW
        });
    }
    state.by_term.retain(|_, m| !m.is_empty());
    state.updated = now;
}

/// Record one pick at `now`: decay to `now` with the state's half-life,
/// then add 1.0 to the face (and to the face under its concept).
pub fn record(state: &mut UsageState, pick: &Pick, now: u64) {
    let hl = state.half_life_days;
    decay(state, now, hl);
    if state.updated < now {
        state.updated = now;
    }
    *state.global.entry(pick.id).or_insert(0.0) += 1.0;
    if let Some(k) = pick.term_key.as_deref().filter(|k| !k.is_empty()) {
        *state.by_term.entry(k.to_string()).or_default().entry(pick.id).or_insert(0.0) += 1.0;
    }
}

/// Keep at most `max_entries` (term, face) pairs (global entries count too),
/// evicting the lowest weights first; ties evict the larger id / later term
/// first, so the result never depends on map order.
pub fn prune(state: &mut UsageState, max_entries: usize) {
    if state.len() <= max_entries {
        return;
    }
    // (weight, term or None for global, id)
    let mut all: Vec<(f32, Option<String>, FaceId)> = Vec::with_capacity(state.len());
    all.extend(state.global.iter().map(|(id, w)| (*w, None, *id)));
    for (k, m) in &state.by_term {
        all.extend(m.iter().map(|(id, w)| (*w, Some(k.clone()), *id)));
    }
    // strongest first
    all.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, k, id) in all.into_iter().skip(max_entries) {
        match k {
            None => {
                state.global.remove(&id);
            }
            Some(k) => {
                if let Some(m) = state.by_term.get_mut(&k) {
                    m.remove(&id);
                }
            }
        }
    }
    state.by_term.retain(|_, m| !m.is_empty());
}

/// The map a query takes: each weight `w` becomes `w / (w + 1)`, so one
/// recent pick is 0.5, three are 0.75, and nothing exceeds 1.
pub fn to_map(state: &UsageState) -> UsageMap {
    let squash = |w: &f32| if w.is_nan() || *w <= 0.0 { 0.0 } else { w / (w + 1.0) };
    UsageMap {
        global: state.global.iter().map(|(k, w)| (*k, squash(w))).collect(),
        by_term: state
            .by_term
            .iter()
            .map(|(t, m)| (t.clone(), m.iter().map(|(k, w)| (*k, squash(w))).collect()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    fn id(t: &str) -> FaceId {
        FaceId::of_text(t)
    }

    #[test]
    fn record_adds_global_and_term() {
        let mut s = UsageState::default();
        record(&mut s, &Pick::new(id("a"), Some("idk".into())), 1000);
        record(&mut s, &Pick::new(id("a"), None), 1000);
        assert_eq!(s.global[&id("a")], 2.0);
        assert_eq!(s.by_term["idk"][&id("a")], 1.0);
        assert_eq!(s.updated, 1000);
    }

    #[test]
    fn decay_halves_after_one_half_life() {
        let mut s = UsageState::new(30.0);
        record(&mut s, &Pick::new(id("a"), Some("idk".into())), 0);
        decay(&mut s, 30 * DAY, 30.0);
        assert!((s.global[&id("a")] - 0.5).abs() < 1e-6);
        assert!((s.by_term["idk"][&id("a")] - 0.5).abs() < 1e-6);
        // a pick later decays the old weight first
        record(&mut s, &Pick::new(id("a"), None), 60 * DAY);
        assert!((s.global[&id("a")] - 1.25).abs() < 1e-5);
    }

    #[test]
    fn decay_forgets_faded_weights_and_ignores_clock_going_back() {
        let mut s = UsageState::new(1.0);
        record(&mut s, &Pick::new(id("a"), Some("x".into())), 100 * DAY);
        decay(&mut s, 50 * DAY, 1.0);
        assert_eq!(s.global.len(), 1);
        decay(&mut s, 120 * DAY, 1.0);
        assert!(s.is_empty());
    }

    #[test]
    fn to_map_squashes_into_unit_range() {
        let mut s = UsageState::default();
        let p = Pick::new(id("a"), Some("shrug".into()));
        record(&mut s, &p, 5);
        let m = to_map(&s);
        assert_eq!(m.global[&id("a")], 0.5);
        for _ in 0..2 {
            record(&mut s, &p, 5);
        }
        let m = to_map(&s);
        assert_eq!(m.global[&id("a")], 0.75);
        assert_eq!(m.weight(id("a"), Some("shrug")), (0.75 + 4.0 * 0.75) / 5.0);
        assert_eq!(m.weight(id("a"), Some("other")), 0.75 / 5.0);
        assert_eq!(m.weight(id("b"), None), 0.0);
    }

    #[test]
    fn prune_keeps_the_strongest() {
        let mut s = UsageState::default();
        for (i, t) in ["a", "b", "c"].iter().enumerate() {
            for _ in 0..=i {
                record(&mut s, &Pick::new(id(t), None), 0);
            }
        }
        prune(&mut s, 2);
        assert_eq!(s.global.len(), 2);
        assert!(!s.global.contains_key(&id("a")));
    }

    #[test]
    fn serde_round_trip() {
        let mut s = UsageState::default();
        record(&mut s, &Pick::new(id("a"), Some("idk".into())), 7);
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<UsageState>(&j).unwrap(), s);
        let m = to_map(&s);
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<UsageMap>(&j).unwrap(), m);
    }
}
