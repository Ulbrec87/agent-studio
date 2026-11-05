# Releasing

## Cutting a release

1. Make sure `main` is green (CI: fmt, clippy `-D warnings`, tests on Linux, macOS and Windows).
2. Set the version in `Cargo.toml` (`[package]` and `[workspace.package]`) and in the `version` of the path dependencies
   (`as-core = { version = ... }`, ...), run `cargo build` so `Cargo.lock` follows, merge that through a pull request.
3. Tag the merge commit and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`. The tag must equal the crate version
   (the workflow checks it). A tag with a hyphen (`v0.2.0-rc.1`) is published as a prerelease, which the in-app update check ignores.

The `Release` workflow (`.github/workflows/release.yml`) then:

- builds `cargo build --release --locked` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (arm runner),
  `x86_64-apple-darwin` and `aarch64-apple-darwin` (both on the Apple Silicon runner), `x86_64-pc-windows-msvc`;
- runs `agent-studio --version` on every binary it can execute;
- packs `agent-studio-<tag>-<target>.tar.gz` (`.zip` on Windows) with the binary, `README.md` and `LICENSE`, plus a `.sha256` per archive;
- (tags only) creates the GitHub Release with generated notes and attaches the archives and `SHA256SUMS`.

**Dry run**: *Actions > Release > Run workflow* on any branch builds and packs everything and keeps the archives as workflow artifacts
(14 days) without publishing anything. Do this after changing the workflow or the build profile.

The update check (`as_core::updates`) picks the archive by the target triple and extension in the asset name: keep the naming.

## Not automated yet (future work)

Hydra updates Homebrew, AUR and winget from its release workflow (`hydra/.github/workflows/update-*.yml`, `hydra/packaging/`). Agent
Studio does not, for now. When it makes sense:

- **Homebrew**: a tap (`kawashita86/homebrew-tap`) with a formula that downloads the macOS / Linux archive and checks the SHA256 from
  `SHA256SUMS`; the update check already recognises `brew upgrade agent-studio`.
- **winget / Scoop / Chocolatey**: manifests that point at the Windows `.zip` (portable package); the hints `winget upgrade`, `scoop update`
  and `choco upgrade` are already what the update banner says for such installs.
- **AUR**: a `agent-studio-bin` PKGBUILD over the Linux archives.
- **crates.io**: publish the workspace crates (they are path dependencies today) so `cargo install agent-studio --locked` works, then switch
  the hint in `as_core::updates::upgrade_hint`.
- Code signing / notarization of the macOS and Windows binaries (not done: Gatekeeper and SmartScreen will warn on downloaded archives; a
  `cargo install` build is not affected).
