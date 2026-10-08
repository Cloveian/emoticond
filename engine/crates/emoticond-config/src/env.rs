//! A snapshot of environment variables.
//!
//! Everything in this crate reads the environment through [`Env`], never
//! through `std::env` directly, so tests and embedders can pass exactly the
//! variables they mean.

use std::collections::BTreeMap;

/// Environment variables, captured once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: BTreeMap<String, String>,
}

impl Env {
    /// No variables at all.
    pub fn empty() -> Env {
        Env::default()
    }

    /// The current process environment. Variables whose name or value is
    /// not valid UTF-8 are skipped.
    pub fn from_process() -> Env {
        Env { vars: std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect() }
    }

    /// Builder-style: add or replace one variable.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Env {
        self.vars.insert(name.into(), value.into());
        self
    }

    /// Add or replace one variable.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }

    /// The value of `name`, if set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// The value of `name` if set and not empty or blank.
    pub fn get_nonempty(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|v| !v.trim().is_empty())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Env {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Env {
        Env { vars: iter.into_iter().map(|(k, v)| (k.into(), v.into())).collect() }
    }
}
