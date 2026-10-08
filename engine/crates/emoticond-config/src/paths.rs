//! Where config, policy, data and state live (docs/options.md §7.1).
//!
//! [`Paths`] is plain data: every directory resolved once, with derived
//! file locations as methods. The state crate (`emoticond-state`) takes these
//! paths as arguments; it never resolves them itself.
//!
//! - **Linux and other Unix:** the XDG base directory spec. Relative values
//!   of `$XDG_*` are ignored, as the spec requires.
//! - **macOS:** `~/Library/Application Support/emoticond` (config, data,
//!   state under `state/`), `~/Library/Caches/emoticond`, and
//!   `/Library/Application Support/emoticond` for the system config, data and
//!   policy.
//! - **Windows:** `%APPDATA%\emoticond` (config), `%LOCALAPPDATA%\emoticond\{data,
//!   state,cache}`, and `%ProgramData%\emoticond` for the system config, data
//!   and policy.
//!
//! The policy file is a fixed path (`/etc/emoticond/policy.toml` on Unix): it
//! doesn't follow `$XDG_CONFIG_DIRS`, so a user can't redirect it.

use crate::env::Env;
use std::path::{Path, PathBuf};

/// The app directory name everywhere.
pub const APP: &str = "emoticond";

/// Which convention to resolve paths with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Platform {
    /// XDG base directories (Linux, BSD, other Unix).
    Xdg,
    MacOs,
    Windows,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Xdg
        }
    }

    fn list_sep(self) -> char {
        if self == Platform::Windows {
            ';'
        } else {
            ':'
        }
    }

    /// Split a path list (`$XDG_DATA_DIRS`, `EMOTICOND_DATA`) with this
    /// platform's separator, dropping empty entries.
    pub fn split_list(self, s: &str) -> Vec<PathBuf> {
        s.split(self.list_sep()).map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from).collect()
    }
}

/// Every location the config crate and the state crate need, resolved.
///
/// Fields are public so a front-end or a test can override any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Paths {
    /// Platform the paths were resolved for (path-list separator, etc.).
    pub platform: Platform,
    /// The user's home, for `~` in config files. `None` if unknown.
    pub home: Option<PathBuf>,
    /// User config dir: `$XDG_CONFIG_HOME/emoticond` (`config.toml`,
    /// `overlays/`).
    pub config_home: PathBuf,
    /// System config dirs, **most important first**: each
    /// `$XDG_CONFIG_DIRS/emoticond` (`/etc/xdg/emoticond`).
    pub config_dirs: Vec<PathBuf>,
    /// Data search path, in order: `$XDG_DATA_HOME/emoticond`, each
    /// `$XDG_DATA_DIRS/emoticond`, then `<exe>/../share/emoticond`.
    pub data_dirs: Vec<PathBuf>,
    /// State dir: `$XDG_STATE_HOME/emoticond` (usage, report queue, machine
    /// overlays, user blocklist).
    pub state_dir: PathBuf,
    /// Cache dir: `$XDG_CACHE_HOME/emoticond` (downloaded blocklists).
    pub cache_dir: PathBuf,
    /// The packager/admin policy: `/etc/emoticond/policy.toml`.
    pub policy_file: PathBuf,
    /// The packager/admin blocklist: `/etc/emoticond/blocklist.txt`.
    pub system_blocklist: PathBuf,
}

impl Paths {
    /// Resolve for this process: [`Platform::current`], the process
    /// environment, the home dir and the running executable.
    pub fn detect() -> Paths {
        let env = Env::from_process();
        Paths::detect_with(&env)
    }

    /// Like [`detect`](Paths::detect) but reading variables from `env`.
    pub fn detect_with(env: &Env) -> Paths {
        #[allow(deprecated)] // undeprecated in Rust 1.86; fine on every platform we target
        let home = std::env::home_dir();
        let exe = std::env::current_exe().ok();
        Paths::resolve(Platform::current(), env, home.as_deref(), exe.as_deref())
    }

    /// Resolve from explicit inputs (no process state is read). `home`
    /// falls back to `$HOME` (`%USERPROFILE%` on Windows).
    pub fn resolve(platform: Platform, env: &Env, home: Option<&Path>, exe: Option<&Path>) -> Paths {
        let home: Option<PathBuf> = home.map(Path::to_path_buf).or_else(|| {
            let var = if platform == Platform::Windows { "USERPROFILE" } else { "HOME" };
            env.get_nonempty(var).map(PathBuf::from)
        });
        let home_or_dot = || home.clone().unwrap_or_else(|| PathBuf::from("."));
        let exe_share = exe
            .and_then(|e| e.parent())
            .and_then(|bin| bin.parent())
            .map(|prefix| prefix.join("share").join(APP));
        let mut p = match platform {
            Platform::Xdg => {
                let abs = |var: &str| env.get_nonempty(var).map(PathBuf::from).filter(|p| p.is_absolute());
                let list = |var: &str, dflt: &[&str]| -> Vec<PathBuf> {
                    let v: Vec<PathBuf> = env
                        .get_nonempty(var)
                        .map(|s| platform.split_list(s).into_iter().filter(|p| p.is_absolute()).collect())
                        .unwrap_or_default();
                    if v.is_empty() {
                        dflt.iter().map(PathBuf::from).collect()
                    } else {
                        v
                    }
                };
                let config_home = abs("XDG_CONFIG_HOME").unwrap_or_else(|| home_or_dot().join(".config"));
                let data_home = abs("XDG_DATA_HOME").unwrap_or_else(|| home_or_dot().join(".local/share"));
                let state_home = abs("XDG_STATE_HOME").unwrap_or_else(|| home_or_dot().join(".local/state"));
                let cache_home = abs("XDG_CACHE_HOME").unwrap_or_else(|| home_or_dot().join(".cache"));
                let mut data_dirs = vec![data_home.join(APP)];
                data_dirs.extend(list("XDG_DATA_DIRS", &["/usr/local/share", "/usr/share"]).iter().map(|d| d.join(APP)));
                Paths {
                    platform,
                    home: home.clone(),
                    config_home: config_home.join(APP),
                    config_dirs: list("XDG_CONFIG_DIRS", &["/etc/xdg"]).iter().map(|d| d.join(APP)).collect(),
                    data_dirs,
                    state_dir: state_home.join(APP),
                    cache_dir: cache_home.join(APP),
                    policy_file: PathBuf::from("/etc/emoticond/policy.toml"),
                    system_blocklist: PathBuf::from("/etc/emoticond/blocklist.txt"),
                }
            }
            Platform::MacOs => {
                let support = home_or_dot().join("Library/Application Support").join(APP);
                let system = PathBuf::from("/Library/Application Support").join(APP);
                Paths {
                    platform,
                    home: home.clone(),
                    config_home: support.clone(),
                    config_dirs: vec![system.clone()],
                    data_dirs: vec![support.join("data"), system.join("data")],
                    state_dir: support.join("state"),
                    cache_dir: home_or_dot().join("Library/Caches").join(APP),
                    policy_file: system.join("policy.toml"),
                    system_blocklist: system.join("blocklist.txt"),
                }
            }
            Platform::Windows => {
                let roaming = env
                    .get_nonempty("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home_or_dot().join("AppData").join("Roaming"));
                let local = env
                    .get_nonempty("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home_or_dot().join("AppData").join("Local"));
                let program = env.get_nonempty("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
                let system = program.join(APP);
                Paths {
                    platform,
                    home: home.clone(),
                    config_home: roaming.join(APP),
                    config_dirs: vec![system.clone()],
                    data_dirs: vec![local.join(APP).join("data"), system.join("data")],
                    state_dir: local.join(APP).join("state"),
                    cache_dir: local.join(APP).join("cache"),
                    policy_file: system.join("policy.toml"),
                    system_blocklist: system.join("blocklist.txt"),
                }
            }
        };
        if let Some(s) = exe_share {
            p.data_dirs.push(s);
        }
        dedup(&mut p.data_dirs);
        dedup(&mut p.config_dirs);
        p
    }

    /// Everything under one directory, nothing read from the process. For
    /// tests and sandboxed embedders:
    /// `root/config/emoticond`, `root/etc/xdg/emoticond`, `root/data/emoticond`,
    /// `root/state/emoticond`, `root/cache/emoticond`, `root/etc/emoticond/policy.toml`,
    /// home `root/home`.
    pub fn rooted(root: impl AsRef<Path>) -> Paths {
        let r = root.as_ref();
        Paths {
            platform: Platform::current(),
            home: Some(r.join("home")),
            config_home: r.join("config").join(APP),
            config_dirs: vec![r.join("etc/xdg").join(APP)],
            data_dirs: vec![r.join("data").join(APP)],
            state_dir: r.join("state").join(APP),
            cache_dir: r.join("cache").join(APP),
            policy_file: r.join("etc/emoticond/policy.toml"),
            system_blocklist: r.join("etc/emoticond/blocklist.txt"),
        }
    }

    /// `config_home/config.toml`.
    pub fn user_config(&self) -> PathBuf {
        self.config_home.join("config.toml")
    }

    /// `config.toml` in each system config dir, most important first.
    pub fn system_configs(&self) -> Vec<PathBuf> {
        self.config_dirs.iter().map(|d| d.join("config.toml")).collect()
    }

    /// Hand-written overlays: `config_home/overlays/`.
    pub fn user_overlays_dir(&self) -> PathBuf {
        self.config_home.join("overlays")
    }

    /// Machine-written overlays (from feedback): `state_dir/overlays/`.
    pub fn state_overlays_dir(&self) -> PathBuf {
        self.state_dir.join("overlays")
    }

    /// Default popularity store: `state_dir/usage.json`.
    pub fn usage_file(&self) -> PathBuf {
        self.state_dir.join("usage.json")
    }

    /// Default report queue: `state_dir/reports/queue.jsonl`.
    /// The answer to "send reports?" (`consent.json`).
    pub fn consent_file(&self) -> PathBuf {
        self.state_dir.join("consent.json")
    }

    pub fn reports_queue(&self) -> PathBuf {
        self.state_dir.join("reports").join("queue.jsonl")
    }

    /// Sent reports: `state_dir/reports/sent.jsonl`.
    pub fn reports_sent(&self) -> PathBuf {
        self.state_dir.join("reports").join("sent.jsonl")
    }

    /// Shared-popularity outbox: `state_dir/outbox/`.
    pub fn outbox_dir(&self) -> PathBuf {
        self.state_dir.join("outbox")
    }

    /// The user's blocklist (offensive reports): `state_dir/blocklist.txt`.
    pub fn state_blocklist(&self) -> PathBuf {
        self.state_dir.join("blocklist.txt")
    }

    /// The user's hand-written blocklist: `config_home/overlays/blocklist.txt`.
    pub fn user_blocklist(&self) -> PathBuf {
        self.user_overlays_dir().join("blocklist.txt")
    }
}

pub(crate) fn dedup(v: &mut Vec<PathBuf>) {
    let mut seen = std::collections::BTreeSet::new();
    v.retain(|p| seen.insert(p.clone()));
}

/// Expand a leading `~` with `home`; make a relative path relative to
/// `base` (the directory of the file it came from), if given.
pub(crate) fn expand(s: &str, home: Option<&Path>, base: Option<&Path>) -> PathBuf {
    let p = if s == "~" {
        home.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(s))
    } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        match home {
            Some(h) => h.join(rest),
            None => PathBuf::from(s),
        }
    } else {
        PathBuf::from(s)
    };
    match base {
        Some(b) if p.is_relative() => b.join(p),
        _ => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_defaults_from_home() {
        let p = Paths::resolve(Platform::Xdg, &Env::empty(), Some(Path::new("/home/u")), Some(Path::new("/usr/bin/emoticond")));
        assert_eq!(p.config_home, Path::new("/home/u/.config/emoticond"));
        assert_eq!(p.user_config(), Path::new("/home/u/.config/emoticond/config.toml"));
        assert_eq!(p.config_dirs, [Path::new("/etc/xdg/emoticond")]);
        assert_eq!(
            p.data_dirs,
            [Path::new("/home/u/.local/share/emoticond"), Path::new("/usr/local/share/emoticond"), Path::new("/usr/share/emoticond")],
            "exe-relative /usr/share/emoticond is deduplicated"
        );
        assert_eq!(p.state_dir, Path::new("/home/u/.local/state/emoticond"));
        assert_eq!(p.cache_dir, Path::new("/home/u/.cache/emoticond"));
        assert_eq!(p.policy_file, Path::new("/etc/emoticond/policy.toml"));
        assert_eq!(p.usage_file(), Path::new("/home/u/.local/state/emoticond/usage.json"));
        assert_eq!(p.reports_queue(), Path::new("/home/u/.local/state/emoticond/reports/queue.jsonl"));
    }

    #[test]
    fn xdg_vars_win_and_relative_ones_are_ignored() {
        let env = Env::empty()
            .with("HOME", "/h")
            .with("XDG_CONFIG_HOME", "/c")
            .with("XDG_DATA_HOME", "relative/ignored")
            .with("XDG_STATE_HOME", "/s")
            .with("XDG_CONFIG_DIRS", "/sys1:relative:/sys2")
            .with("XDG_DATA_DIRS", "/d1:/d2");
        let p = Paths::resolve(Platform::Xdg, &env, None, Some(Path::new("/opt/kao/bin/emoticond")));
        assert_eq!(p.config_home, Path::new("/c/emoticond"));
        assert_eq!(p.config_dirs, [Path::new("/sys1/emoticond"), Path::new("/sys2/emoticond")]);
        assert_eq!(
            p.data_dirs,
            [
                Path::new("/h/.local/share/emoticond"),
                Path::new("/d1/emoticond"),
                Path::new("/d2/emoticond"),
                Path::new("/opt/kao/share/emoticond")
            ]
        );
        assert_eq!(p.state_dir, Path::new("/s/emoticond"));
        assert_eq!(p.policy_file, Path::new("/etc/emoticond/policy.toml"), "policy never follows XDG");
    }

    #[test]
    fn macos_and_windows() {
        let p = Paths::resolve(Platform::MacOs, &Env::empty(), Some(Path::new("/Users/u")), None);
        assert_eq!(p.user_config(), Path::new("/Users/u/Library/Application Support/emoticond/config.toml"));
        assert_eq!(p.policy_file, Path::new("/Library/Application Support/emoticond/policy.toml"));
        assert_eq!(p.cache_dir, Path::new("/Users/u/Library/Caches/emoticond"));
        let env = Env::empty().with("APPDATA", "C:/U/R").with("LOCALAPPDATA", "C:/U/L").with("ProgramData", "C:/PD");
        let p = Paths::resolve(Platform::Windows, &env, Some(Path::new("C:/U")), None);
        assert_eq!(p.config_home, Path::new("C:/U/R/emoticond"));
        assert_eq!(p.state_dir, Path::new("C:/U/L/emoticond/state"));
        assert_eq!(p.policy_file, Path::new("C:/PD/emoticond/policy.toml"));
        assert_eq!(Platform::Windows.split_list("a;b;;c").len(), 3);
    }

    #[test]
    fn tilde_and_relative_expansion() {
        let h = Some(Path::new("/home/u"));
        assert_eq!(expand("~/x", h, None), Path::new("/home/u/x"));
        assert_eq!(expand("~", h, None), Path::new("/home/u"));
        assert_eq!(expand("rel/x", h, Some(Path::new("/etc/xdg/emoticond"))), Path::new("/etc/xdg/emoticond/rel/x"));
        assert_eq!(expand("/abs", h, Some(Path::new("/b"))), Path::new("/abs"));
    }
}
