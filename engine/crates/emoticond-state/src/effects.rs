//! Local effects of a report (docs/options.md §5.1 step 3).
//!
//! The core says what a report does locally ([`LocalEffect`]); this applies
//! it to the files:
//! - `Block`: the face goes into the state `blocklist.txt`. **Always**, even
//!   with `feedback.apply_locally = false` and even when sending is off.
//! - `RecordPick`: a pick in the [`UsageStore`] (if there is one and its
//!   mode is not `Off`).
//! - `Demote`: a negative boost in `overlays/boosts.jsonl`.
//!
//! The front-end then passes the blocked ids through `exclude` at once and
//! calls `set_blocklist` / `reload_overlays`; other processes see the change
//! on their next [`Shared::refresh`](crate::Shared::refresh).

use crate::blocklist::add_to_blocklist_noted;
use crate::error::Result;
use crate::overlay::demote;
use crate::paths::StatePaths;
use crate::usage::UsageStore;
use emoticond::{FaceId, LocalEffect, Pick};

/// What [`apply_local_effects`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Applied {
    /// Faces now in the blocklist (newly or already): exclude them at once.
    pub blocked: Vec<FaceId>,
    /// Picks recorded in the usage store.
    pub picked: Vec<FaceId>,
    /// (term_key, face) pairs demoted.
    pub demoted: Vec<(String, FaceId)>,
    /// Effects not applied: local effects off, no usage store, popularity
    /// off, or an effect this version does not know.
    pub skipped: Vec<LocalEffect>,
}

/// Where local effects go.
#[derive(Debug)]
pub struct LocalEffects<'a> {
    pub paths: &'a StatePaths,
    /// The usage store for "really good fit" picks; `None` skips them.
    pub usage: Option<&'a mut UsageStore>,
    /// `feedback.apply_locally` (default true). The offensive hide applies
    /// regardless.
    pub apply_locally: bool,
}

impl<'a> LocalEffects<'a> {
    pub fn new(paths: &'a StatePaths, usage: Option<&'a mut UsageStore>, apply_locally: bool) -> LocalEffects<'a> {
        LocalEffects { paths, usage, apply_locally }
    }

    /// Apply `effects` at `now`. Mandatory effects (the offensive hide) run
    /// first. Every effect is attempted; if any failed, the first error is
    /// returned after the rest were tried.
    pub fn apply(&mut self, effects: &[LocalEffect], now: u64) -> Result<Applied> {
        let mut out = Applied::default();
        let mut first_err = None;
        let ordered = effects.iter().filter(|e| e.is_mandatory()).chain(effects.iter().filter(|e| !e.is_mandatory()));
        for e in ordered {
            if !e.is_mandatory() && !self.apply_locally {
                out.skipped.push(e.clone());
                continue;
            }
            let r = match e {
                LocalEffect::Block { id, text } => {
                    add_to_blocklist_noted(&self.paths.blocklist, *id, text.as_deref()).map(|_| out.blocked.push(*id))
                }
                LocalEffect::RecordPick { id, term_key } => match self.usage.as_deref_mut() {
                    Some(u) => u.record(&Pick::new(*id, term_key.clone()), now).map(|recorded| {
                        if recorded {
                            out.picked.push(*id)
                        } else {
                            out.skipped.push(e.clone())
                        }
                    }),
                    None => {
                        out.skipped.push(e.clone());
                        Ok(())
                    }
                },
                LocalEffect::Demote { id, term_key } => {
                    demote(&self.paths.boosts, term_key, *id).map(|_| out.demoted.push((term_key.clone(), *id)))
                }
                _ => {
                    out.skipped.push(e.clone());
                    Ok(())
                }
            };
            if let Err(err) = r {
                first_err.get_or_insert(err);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }
}

/// Apply `effects` (see [`LocalEffects::apply`]).
pub fn apply_local_effects(
    paths: &StatePaths,
    usage: Option<&mut UsageStore>,
    effects: &[LocalEffect],
    apply_locally: bool,
    now: u64,
) -> Result<Applied> {
    LocalEffects::new(paths, usage, apply_locally).apply(effects, now)
}
