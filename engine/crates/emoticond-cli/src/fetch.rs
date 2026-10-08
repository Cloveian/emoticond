//! `emoticond data fetch [VERSION] [--set core|full|lite] [--dir DIR]`:
//! download a data file from the emoticond-data repository (pipeline/
//! publish_data.py lays it out: `X.Y/` per version and `X/` for the newest
//! `X.*`, each with the three sets and a `SHA256SUMS`), check it against
//! that list, and put it in the user data dir, where emoticond looks first.
//! Without a VERSION it takes the newest data this build can read
//! (`emoticond::DATA_COMPAT`).

use super::cli::{Args, IO, OK, USAGE};

/// Where the files are, one folder per version (`$EMOTICOND_DATA_URL`
/// overrides it, for a mirror).
pub const DATA_URL: &str = "https://raw.githubusercontent.com/Cloveian/emoticond-data/main";

const SETS: &[&str] = &["core", "full", "lite"];

pub fn data(a: &Args) -> i32 {
    match a.pos.first().map(String::as_str) {
        Some("fetch") => fetch(a),
        Some(other) => {
            eprintln!("emoticond data: unknown subcommand {other:?} (fetch)");
            USAGE
        }
        None => {
            eprintln!("usage: emoticond data fetch [VERSION] [--set core|full|lite] [--dir DIR]");
            USAGE
        }
    }
}

#[cfg(not(feature = "net"))]
fn fetch(_: &Args) -> i32 {
    eprintln!("emoticond data fetch: this build has no network code (built without the `net` feature)");
    eprintln!("download a .kmj by hand from {DATA_URL}/{}/ into ~/.local/share/emoticond/", emoticond::DATA_COMPAT);
    USAGE
}

#[cfg(feature = "net")]
fn fetch(a: &Args) -> i32 {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let set = a.value("--set").unwrap_or("core");
    if !SETS.contains(&set) {
        eprintln!("emoticond data fetch: --set is core, full or lite");
        return USAGE;
    }
    let version = a.pos.get(1).map(String::as_str).unwrap_or(emoticond::DATA_COMPAT);
    let valid = !version.is_empty() && version.split('.').count() <= 2 && version.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !valid {
        eprintln!("emoticond data fetch: a version is X.Y (like 1.0), or X for the newest X.*");
        return USAGE;
    }
    if version.split('.').next() != Some(emoticond::DATA_COMPAT) {
        eprintln!("emoticond data fetch: this emoticond ({}) reads data {}.x, not {version}", emoticond::VERSION, emoticond::DATA_COMPAT);
        return USAGE;
    }
    let dir = match a.value("--dir") {
        Some(d) => std::path::PathBuf::from(d),
        None => match emoticond_config::Paths::detect().data_dirs.first() {
            Some(d) => d.clone(),
            None => {
                eprintln!("emoticond data fetch: no data directory; give --dir");
                return USAGE;
            }
        },
    };
    let base = std::env::var("EMOTICOND_DATA_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DATA_URL.to_string());
    let base = format!("{}/{version}", base.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .user_agent(concat!("emoticond/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let get = |url: &str| -> Result<Vec<u8>, String> {
        let mut resp = agent.get(url).call().map_err(|e| match e {
            ureq::Error::StatusCode(404) => format!("{url}: not found (no such version or set?)"),
            e => format!("{url}: {e}"),
        })?;
        let mut body = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(512 * 1024 * 1024)
            .read_to_end(&mut body)
            .map_err(|e| format!("{url}: {e}"))?;
        Ok(body)
    };

    let file = format!("{set}.kmj");
    let sums = match get(&format!("{base}/SHA256SUMS")) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            eprintln!("emoticond data fetch: {e}");
            return IO;
        }
    };
    let Some(want) = sums.lines().find_map(|l| l.split_once("  ").filter(|(_, f)| f.trim() == file).map(|(h, _)| h.trim().to_string())) else {
        eprintln!("emoticond data fetch: {base}/SHA256SUMS does not list {file}");
        return IO;
    };
    eprintln!("downloading {base}/{file} …");
    let bytes = match get(&format!("{base}/{file}")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("emoticond data fetch: {e}");
            return IO;
        }
    };
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        eprintln!("emoticond data fetch: {file} does not match its checksum (got {got}, want {want}); nothing written");
        return IO;
    }
    let dest = dir.join(&file);
    let written = std::fs::create_dir_all(&dir).and_then(|_| {
        let tmp = dir.join(format!(".{file}.part"));
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &dest)
    });
    if let Err(e) = written {
        eprintln!("emoticond data fetch: writing {}: {e}", dest.display());
        return IO;
    }
    println!("{} ({:.1} MB, {version})", dest.display(), bytes.len() as f64 / 1e6);
    if set != "core" && dir.join("core.kmj").exists() {
        eprintln!("note: core.kmj is also there and is found first; remove it, or set EMOTICOND_DATA_FILE={}", dest.display());
    }
    OK
}
