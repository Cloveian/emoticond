//! Walking a TOML table (a config file, a profile, a daemon request) into
//! validated setting entries plus warnings.

use crate::codes;
use crate::config::Config;
use crate::load::Source;
use crate::schema::{self, SPECS};
use crate::value::{Bad, Ctx, Kind, Value};
use crate::ConfigError;
use emoticond::Warning;
use std::collections::BTreeMap;
use std::path::Path;

/// One value set by one layer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub key: &'static str,
    pub value: Value,
    pub source: Source,
}

pub(crate) struct Walker<'a> {
    pub ctx: Ctx<'a>,
    /// Prefix for warning messages (`/etc/xdg/emoticond/config.toml: `).
    pub label: String,
    /// Daemon requests: bad values are errors, not warnings.
    pub strict: bool,
    pub source: Source,
    pub entries: Vec<Entry>,
    pub warnings: Vec<Warning>,
    pub errors: Vec<ConfigError>,
}

impl<'a> Walker<'a> {
    pub fn new(ctx: Ctx<'a>, label: String, strict: bool, source: Source) -> Walker<'a> {
        Walker { ctx, label, strict, source, entries: Vec::new(), warnings: Vec::new(), errors: Vec::new() }
    }

    fn warn(&mut self, code: &'static str, key: &str, msg: impl std::fmt::Display) {
        self.warnings.push(Warning::new(code, Some(key), format!("{}{msg}", self.label)));
    }

    /// Walk every key of `t` under `prefix` (`""` for a whole file).
    pub fn table(&mut self, prefix: &str, t: &toml::Table) {
        for (k, v) in t {
            let path = if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
            self.value(&path, v);
        }
    }

    pub fn value(&mut self, path: &str, v: &toml::Value) {
        if let Some(sp) = schema::spec(path) {
            if matches!(v, toml::Value::Table(_)) && !matches!(sp.kind, Kind::EmotionMap | Kind::Weights) {
                let msg = format!("`{path}` expects {}, not a table; ignored", sp.kind.describe());
                if self.strict {
                    self.errors.push(ConfigError::InvalidValue { key: path.to_string(), message: msg });
                } else {
                    self.warn(codes::INVALID_VALUE, path, msg);
                }
                return;
            }
            self.parsed(sp.key, sp.kind, v);
        } else if let (toml::Value::Table(t), true) = (v, SPECS.iter().any(|s| s.key.starts_with(&format!("{path}.")))) {
            self.table(path, t);
        } else {
            self.warn(codes::UNKNOWN_KEY, path, format!("unknown option `{path}`; ignored"));
        }
    }

    fn parsed(&mut self, key: &'static str, kind: Kind, v: &toml::Value) {
        match kind.parse_toml(v, self.ctx) {
            Ok(p) => {
                if let Some(note) = p.clamped {
                    self.warn(codes::OUT_OF_RANGE, key, format!("`{key}`: {note}"));
                }
                self.entries.push(Entry { key, value: p.value, source: self.source.clone() });
            }
            Err(bad) if self.strict => {
                self.errors.push(ConfigError::InvalidValue { key: key.to_string(), message: bad.message().to_string() })
            }
            Err(Bad::Type(m)) => self.warn(codes::INVALID_VALUE, key, format!("`{key}`: {m}; ignored")),
            Err(Bad::Enum(m)) => {
                // options.md §10.1: an unknown enum value means the key's
                // default. On safety the default is strict, the strictest.
                let d = schema::get(&Config::default(), key);
                self.warn(codes::INVALID_VALUE, key, format!("`{key}`: {m}; using the default {d}"));
                self.entries.push(Entry { key, value: d, source: self.source.clone() });
            }
        }
    }

    pub fn finish(self) -> (Vec<Entry>, Vec<Warning>, Vec<ConfigError>) {
        (self.entries, self.warnings, self.errors)
    }
}

/// What one config file says: top-level entries and per-profile entries.
#[derive(Debug, Default)]
pub(crate) struct FileLayers {
    pub top: Vec<Entry>,
    pub profiles: BTreeMap<String, Vec<Entry>>,
}

pub(crate) const CONFIG_VERSION: i64 = 1;

/// Parse one config file's text. Never fails: problems are warnings.
pub(crate) fn config_file(
    text: &str,
    path: &Path,
    home: Option<&Path>,
    system: bool,
    warnings: &mut Vec<Warning>,
) -> Result<FileLayers, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let base = path.parent();
    let ctx = Ctx { home, base };
    let label = format!("{}: ", path.display());
    let top_source = if system { Source::SystemConfig(path.to_path_buf()) } else { Source::UserConfig(path.to_path_buf()) };
    let mut out = FileLayers::default();
    let mut w = Walker::new(ctx, label.clone(), false, top_source);
    for (k, v) in &table {
        match k.as_str() {
            "config_version" => match v.as_integer() {
                Some(n) if n > CONFIG_VERSION => w.warnings.push(Warning::new(
                    codes::NEWER_VERSION,
                    Some("config_version"),
                    format!("{label}config_version {n} is newer than this reader ({CONFIG_VERSION}); reading what it can"),
                )),
                Some(_) => {}
                None => w.warnings.push(Warning::new(codes::INVALID_VALUE, Some("config_version"), format!("{label}config_version must be an integer"))),
            },
            "profile" => match v.as_table() {
                Some(profiles) => {
                    for (name, body) in profiles {
                        let Some(body) = body.as_table() else {
                            w.warnings.push(Warning::new(
                                codes::INVALID_VALUE,
                                Some("profile"),
                                format!("{label}[profile.{name}] must be a table; ignored"),
                            ));
                            continue;
                        };
                        let src = if system {
                            Source::SystemProfile { name: name.to_string(), file: path.to_path_buf() }
                        } else {
                            Source::UserProfile { name: name.to_string(), file: path.to_path_buf() }
                        };
                        let mut pw = Walker::new(ctx, format!("{label}[profile.{name}] "), false, src);
                        pw.table("", body);
                        let (e, ws, _) = pw.finish();
                        warnings.extend(ws);
                        out.profiles.insert(name.to_string(), e);
                    }
                }
                None => w.warnings.push(Warning::new(codes::INVALID_VALUE, Some("profile"), format!("{label}`profile` must be a table"))),
            },
            _ => w.value(k, v),
        }
    }
    let (e, ws, _) = w.finish();
    warnings.extend(ws);
    out.top = e;
    Ok(out)
}

/// JSON (a daemon request's `opts`) → TOML; `null` becomes `"none"` so
/// optional settings can be cleared.
pub(crate) fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    use serde_json::Value as J;
    match v {
        J::Null => toml::Value::String("none".into()),
        J::Bool(b) => toml::Value::Boolean(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        J::String(s) => toml::Value::String(s.clone()),
        J::Array(a) => toml::Value::Array(a.iter().map(json_to_toml).collect()),
        J::Object(o) => toml::Value::Table(o.iter().map(|(k, v)| (k.clone(), json_to_toml(v))).collect()),
    }
}
