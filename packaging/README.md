# packaging

ready-to-publish package files. each release, in order:

1. **tag:** `git tag vX.Y.Z && git push origin vX.Y.Z` → the release
   workflow attaches the binaries and `SHA256SUMS`
2. **crates.io** (from `engine/`, dependencies first):

   ```sh
   cargo publish -p emoticond
   cargo publish -p emoticond-compile
   cargo publish -p emoticond-config
   cargo publish -p emoticond-state
   cargo publish -p emoticond-cli
   ```

   `cargo install emoticond-cli` builds it; `cargo binstall emoticond-cli`
   downloads the release binary (metadata in `crates/emoticond-cli/Cargo.toml`)
3. **AUR:** `aur/emoticond` (builds from source), `aur/emoticond-bin`
   (release binary), `aur/emoticond-data` (the data in
   `/usr/share/emoticond`, for every user). for each:

   ```sh
   git clone ssh://aur@aur.archlinux.org/<pkg>.git
   cp aur/<pkg>/PKGBUILD <pkg>/ && cd <pkg>
   updpkgsums && makepkg --printsrcinfo > .SRCINFO && makepkg -f   # new version: new sums
   git add PKGBUILD .SRCINFO && git commit -m "<version>" && git push
   ```

   `emoticond-data` only changes with a new data version (`X.Y`)
4. **homebrew:** `homebrew/emoticond.rb` goes in a tap repo
   (`Cloveian/homebrew-emoticond`, as `Formula/emoticond.rb`); set `version`
   and the three `sha256` from the release's `SHA256SUMS`

notes:

- `options=('!lto')` in the source PKGBUILD is needed: makepkg's C LTO flags
  break `ring` (TLS for `data fetch` and reports) under Rust's own LTO
- a build without network code: `cargo build --release -p emoticond-cli
  --no-default-features`. it never sends reports and has no `data fetch`
