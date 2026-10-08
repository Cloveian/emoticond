//! Setting values and their types: parsing from TOML and from strings
//! (env vars, `key=value` overrides), range clamping, and display.

use crate::paths::expand;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// One setting's value, as the config crate tracks it per layer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// An optional setting with no value (`max_len = "none"`, a dev log
    /// that is off).
    Unset,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Enum names and URLs.
    Str(String),
    Path(PathBuf),
    Paths(Vec<PathBuf>),
    /// Emotion name → 0..=1.
    Map(BTreeMap<String, f64>),
    /// Tuning weights: emotion, dense, engine, lexical.
    Weights([f64; 4]),
}

impl fmt::Display for Value {
    /// TOML-like rendering, for `emoticond config show`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unset => f.write_str("(unset)"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.abs() < 1e15 {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Path(p) => write!(f, "{:?}", p.display().to_string()),
            Value::Paths(ps) => {
                f.write_str("[")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{:?}", p.display().to_string())?;
                }
                f.write_str("]")
            }
            Value::Map(m) => {
                f.write_str("{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, " {k} = {}", Value::Float(*v))?;
                }
                f.write_str(if m.is_empty() { "}" } else { " }" })
            }
            Value::Weights(w) => write!(
                f,
                "{{ emotion = {}, dense = {}, engine = {}, lexical = {} }}",
                Value::Float(w[0]),
                Value::Float(w[1]),
                Value::Float(w[2]),
                Value::Float(w[3])
            ),
        }
    }
}

/// The type of a setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    /// An integer or `"none"`.
    OptInt { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    /// A number or `"none"`.
    OptFloat { min: f64, max: f64 },
    Enum(&'static [&'static str]),
    /// A path, or `""`/`"off"`/`"none"` for no path.
    OptPath,
    PathList,
    /// An http(s) URL, or `""`/`"none"`.
    OptUrl,
    /// Emotion name → 0..=1.
    EmotionMap,
    Weights,
}

/// Why a value was rejected.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Bad {
    /// Wrong type (`limit = "ten"`): the key is ignored (files) or the
    /// request fails (CLI, daemon).
    Type(String),
    /// A string that isn't one of the enum's values: the key's default
    /// (files) or the request fails.
    Enum(String),
}

impl Bad {
    pub(crate) fn message(&self) -> &str {
        match self {
            Bad::Type(m) | Bad::Enum(m) => m,
        }
    }
}

/// A parsed value plus a note when it had to be clamped.
pub(crate) struct Parsed {
    pub value: Value,
    pub clamped: Option<String>,
}

impl Parsed {
    fn ok(value: Value) -> Result<Parsed, Bad> {
        Ok(Parsed { value, clamped: None })
    }
}

/// How to resolve paths: `~` and the directory of the file the value came
/// from.
#[derive(Clone, Copy, Default)]
pub(crate) struct Ctx<'a> {
    pub home: Option<&'a Path>,
    pub base: Option<&'a Path>,
}

const WEIGHT_NAMES: [&str; 4] = ["emotion", "dense", "engine", "lexical"];
pub(crate) const DEFAULT_WEIGHTS: [f64; 4] = [0.5, 1.2, 1.2, 0.2];

fn type_name(v: &toml::Value) -> &'static str {
    match v {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a number",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a date",
        toml::Value::Array(_) => "a list",
        toml::Value::Table(_) => "a table",
    }
}

fn is_none_word(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "" | "none" | "off" | "unset")
}

fn clamp_int(i: i64, min: i64, max: i64) -> Parsed {
    let to = i.clamp(min, max);
    Parsed { value: Value::Int(to), clamped: (to != i).then(|| format!("{i} is out of range {min}..={max}; using {to}")) }
}

fn clamp_float(x: f64, min: f64, max: f64) -> Result<Parsed, Bad> {
    if !x.is_finite() {
        return Err(Bad::Type(format!("expected a number in {min}..={max}, got {x}")));
    }
    let to = x.clamp(min, max);
    Ok(Parsed { value: Value::Float(to), clamped: (to != x).then(|| format!("{x} is out of range {min}..={max}; using {to}")) })
}

fn parse_bool_str(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn check_url(s: &str) -> Result<Parsed, Bad> {
    let s = s.trim();
    if is_none_word(s) {
        return Parsed::ok(Value::Unset);
    }
    if s.starts_with("https://") || s.starts_with("http://") {
        Parsed::ok(Value::Str(s.to_string()))
    } else {
        Err(Bad::Type(format!("expected an http(s) URL, got {s:?}")))
    }
}

fn clamp_map(m: BTreeMap<String, f64>) -> Parsed {
    let mut notes = Vec::new();
    let m = m
        .into_iter()
        .map(|(k, v)| {
            let to = v.clamp(0.0, 1.0);
            if to != v {
                notes.push(format!("{k} = {v} is out of range 0..=1; using {to}"));
            }
            (k, to)
        })
        .collect();
    Parsed { value: Value::Map(m), clamped: (!notes.is_empty()).then(|| notes.join("; ")) }
}

impl Kind {
    /// Parse a TOML value (config and policy files, daemon requests).
    pub(crate) fn parse_toml(self, v: &toml::Value, ctx: Ctx<'_>) -> Result<Parsed, Bad> {
        use toml::Value as T;
        let wrong = |expected: &str| Bad::Type(format!("expected {expected}, got {}", type_name(v)));
        match (self, v) {
            (Kind::Bool, T::Boolean(b)) => Parsed::ok(Value::Bool(*b)),
            (Kind::Bool, _) => Err(wrong("true or false")),
            (Kind::Int { min, max } | Kind::OptInt { min, max }, T::Integer(i)) => Ok(clamp_int(*i, min, max)),
            (Kind::OptInt { .. } | Kind::OptFloat { .. }, T::String(s)) if is_none_word(s) => Parsed::ok(Value::Unset),
            (Kind::Int { .. } | Kind::OptInt { .. }, _) => Err(wrong("an integer")),
            (Kind::Float { min, max } | Kind::OptFloat { min, max }, T::Float(x)) => clamp_float(*x, min, max),
            (Kind::Float { min, max } | Kind::OptFloat { min, max }, T::Integer(i)) => clamp_float(*i as f64, min, max),
            (Kind::Float { .. } | Kind::OptFloat { .. }, _) => Err(wrong("a number")),
            (Kind::Enum(_), T::String(s)) => self.parse_str(s, ctx),
            (Kind::Enum(names), _) => Err(wrong(&format!("one of {}", names.join(", ")))),
            (Kind::OptPath, T::String(s)) => self.parse_str(s, ctx),
            (Kind::OptPath, T::Boolean(false)) => Parsed::ok(Value::Unset),
            (Kind::OptPath, _) => Err(wrong("a path")),
            (Kind::PathList, T::String(s)) => Parsed::ok(Value::Paths(vec![expand(s, ctx.home, ctx.base)])),
            (Kind::PathList, T::Array(a)) => {
                let mut out = Vec::new();
                for x in a {
                    match x {
                        T::String(s) => out.push(expand(s, ctx.home, ctx.base)),
                        _ => return Err(Bad::Type(format!("expected a list of paths, found {} in it", type_name(x)))),
                    }
                }
                Parsed::ok(Value::Paths(out))
            }
            (Kind::PathList, _) => Err(wrong("a list of paths")),
            (Kind::OptUrl, T::String(s)) => check_url(s),
            (Kind::OptUrl, _) => Err(wrong("a URL")),
            (Kind::EmotionMap, T::Table(t)) => {
                let mut m = BTreeMap::new();
                for (k, x) in t {
                    let f = match x {
                        T::Float(f) if f.is_finite() => *f,
                        T::Integer(i) => *i as f64,
                        _ => return Err(Bad::Type(format!("expected a number for emotion {k:?}, got {}", type_name(x)))),
                    };
                    m.insert(k.to_string(), f);
                }
                Ok(clamp_map(m))
            }
            (Kind::EmotionMap, _) => Err(wrong("a table of emotion = 0..1")),
            (Kind::Weights, T::Table(t)) => {
                let mut w = DEFAULT_WEIGHTS;
                for (k, x) in t {
                    let Some(i) = WEIGHT_NAMES.iter().position(|n| n == k) else {
                        return Err(Bad::Type(format!("unknown weight {k:?} (expected {})", WEIGHT_NAMES.join(", "))));
                    };
                    w[i] = match x {
                        T::Float(f) if f.is_finite() => *f,
                        T::Integer(n) => *n as f64,
                        _ => return Err(Bad::Type(format!("expected a number for weight {k:?}"))),
                    };
                }
                Parsed::ok(Value::Weights(w))
            }
            (Kind::Weights, T::Array(a)) if a.len() == 4 => {
                let mut w = [0.0; 4];
                for (i, x) in a.iter().enumerate() {
                    w[i] = match x {
                        T::Float(f) if f.is_finite() => *f,
                        T::Integer(n) => *n as f64,
                        _ => return Err(Bad::Type("expected four numbers".into())),
                    };
                }
                Parsed::ok(Value::Weights(w))
            }
            (Kind::Weights, T::String(s)) => self.parse_str(s, ctx),
            (Kind::Weights, _) => Err(wrong("{ emotion, dense, engine, lexical } weights")),
        }
    }

    /// Parse a string (env vars and `key=value` overrides).
    pub(crate) fn parse_str(self, s: &str, ctx: Ctx<'_>) -> Result<Parsed, Bad> {
        let t = s.trim();
        match self {
            Kind::Bool => parse_bool_str(t).map(Value::Bool).map(|v| Parsed { value: v, clamped: None }).ok_or_else(|| Bad::Type(format!("expected true or false, got {t:?}"))),
            Kind::Int { min, max } | Kind::OptInt { min, max } => {
                if matches!(self, Kind::OptInt { .. }) && is_none_word(t) {
                    return Parsed::ok(Value::Unset);
                }
                t.parse::<i64>().map(|i| clamp_int(i, min, max)).map_err(|_| Bad::Type(format!("expected an integer, got {t:?}")))
            }
            Kind::Float { min, max } | Kind::OptFloat { min, max } => {
                if matches!(self, Kind::OptFloat { .. }) && is_none_word(t) {
                    return Parsed::ok(Value::Unset);
                }
                let x = t.parse::<f64>().map_err(|_| Bad::Type(format!("expected a number, got {t:?}")))?;
                clamp_float(x, min, max)
            }
            Kind::Enum(names) => {
                let l = t.to_ascii_lowercase();
                match names.iter().find(|n| **n == l) {
                    Some(n) => Parsed::ok(Value::Str((*n).to_string())),
                    None => Err(Bad::Enum(format!("unknown value {t:?} (expected {})", names.join(", ")))),
                }
            }
            Kind::OptPath => {
                if is_none_word(t) {
                    Parsed::ok(Value::Unset)
                } else {
                    Parsed::ok(Value::Path(expand(t, ctx.home, ctx.base)))
                }
            }
            Kind::PathList => {
                let sep = if cfg!(windows) { ';' } else { ':' };
                Parsed::ok(Value::Paths(
                    t.split(sep).map(str::trim).filter(|p| !p.is_empty()).map(|p| expand(p, ctx.home, ctx.base)).collect(),
                ))
            }
            Kind::OptUrl => check_url(t),
            Kind::EmotionMap => {
                let mut m = BTreeMap::new();
                for part in t.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let (k, v) = part
                        .split_once(['=', ':'])
                        .ok_or_else(|| Bad::Type(format!("expected emotion=value pairs, got {part:?}")))?;
                    let v: f64 = v.trim().parse().map_err(|_| Bad::Type(format!("expected a number in {part:?}")))?;
                    if !v.is_finite() {
                        return Err(Bad::Type(format!("expected a number in {part:?}")));
                    }
                    m.insert(k.trim().to_string(), v);
                }
                Ok(clamp_map(m))
            }
            Kind::Weights => {
                let parts: Vec<&str> = t.split(',').map(str::trim).collect();
                let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok().filter(|x| x.is_finite())).collect();
                match nums {
                    Some(n) if n.len() == 4 => Parsed::ok(Value::Weights([n[0], n[1], n[2], n[3]])),
                    _ => Err(Bad::Type(format!("expected four comma-separated numbers (emotion,dense,engine,lexical), got {t:?}"))),
                }
            }
        }
    }

    /// A short description of the accepted values, for settings UIs and
    /// error messages.
    pub(crate) fn describe(self) -> String {
        match self {
            Kind::Bool => "true | false".into(),
            Kind::Int { min, max } => format!("integer {min}..={max}"),
            Kind::OptInt { min, max } => format!("integer {min}..={max}, or \"none\""),
            Kind::Float { min, max } => format!("number {min}..={max}"),
            Kind::OptFloat { min, max } => format!("number {min}..={max}, or \"none\""),
            Kind::Enum(n) => n.join(" | "),
            Kind::OptPath => "path, or \"off\"".into(),
            Kind::PathList => "list of paths".into(),
            Kind::OptUrl => "http(s) URL, or \"none\"".into(),
            Kind::EmotionMap => "table of emotion = 0..1".into(),
            Kind::Weights => "{ emotion, dense, engine, lexical }".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_and_string_parsing() {
        let c = Ctx::default();
        let int = Kind::Int { min: 1, max: 500 };
        let p = int.parse_toml(&toml::Value::Integer(9999), c).ok().unwrap();
        assert_eq!(p.value, Value::Int(500));
        assert!(p.clamped.is_some());
        assert!(matches!(int.parse_toml(&toml::Value::String("ten".into()), c), Err(Bad::Type(_))));
        let e = Kind::Enum(&["strict", "moderate", "off"]);
        assert_eq!(e.parse_str("Moderate", c).ok().unwrap().value, Value::Str("moderate".into()));
        assert!(matches!(e.parse_str("spicy", c), Err(Bad::Enum(_))));
        assert_eq!(Kind::Bool.parse_str("on", c).ok().unwrap().value, Value::Bool(true));
        let of = Kind::OptFloat { min: 1.0, max: 8.0 };
        assert_eq!(of.parse_str("none", c).ok().unwrap().value, Value::Unset);
        assert_eq!(of.parse_toml(&toml::Value::Integer(11), c).ok().unwrap().value, Value::Float(8.0));
        assert_eq!(Kind::OptPath.parse_str("off", c).ok().unwrap().value, Value::Unset);
        assert!(Kind::OptUrl.parse_str("ftp://x", c).is_err());
        let m = Kind::EmotionMap.parse_str("sad=0.3, angry:2", c).ok().unwrap();
        assert_eq!(m.value.to_string(), "{ angry = 1.0, sad = 0.3 }");
        assert!(m.clamped.is_some());
        assert_eq!(Kind::Weights.parse_str("1,2,3,4", c).ok().unwrap().value, Value::Weights([1.0, 2.0, 3.0, 4.0]));
        assert!(Kind::Weights.parse_str("1,2", c).is_err());
    }
}
