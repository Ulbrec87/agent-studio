# Releasing

## Cutting a release

1. Make sure `main` is green (CI job `check (<os>)`: front end, fmt, clippy `-D warnings`, tests on Linux, macOS and Windows).
2. Set the version in the root `Cargo.toml` (`[workspace.package]`), in the `version` of the path dependencies
   (`as-core = { version = ... }`, ...), in `crates/as-gui/tauri.conf.json` (`version`) and in `gui/package.json`; run `cargo build` so
   `Cargo.lock` follows, merge that through a pull request.
3. Tag the merge commit and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`. The tag must equal the workspace version and the
   `tauri.conf.json` version (the workflow checks it). A tag with a hyphen (`v0.2.0-rc.1`) is published as a prerelease, which the
   in-app update check ignores.

The `Release` workflow (`.github/workflows/release.yml`) then, on a runner per platform (WebKit libraries installed on Linux, Node 22,
`npm ci`), runs the Tauri CLI from `gui/node_modules` (`tauri build --target <triple> --bundles <list>`, which also builds the front end):

| Runner | Bundles |
|--------|---------|
| `ubuntu-latest` (x86_64) | AppImage, deb |
| `macos-latest`, target `aarch64-apple-darwin` | dmg (Apple Silicon) |
| `macos-latest`, target `x86_64-apple-darwin` (cross build) | dmg (Intel) |
| `windows-latest` (x86_64) | msi |

- the installers are renamed to `agent-studio-<tag>-<os>-<arch>.<ext>` (no spaces) and get a `.sha256` each; the bundles are **not
  signed or notarized**;
- (tags only) the GitHub Release is created with generated notes and the installers plus `SHA256SUMS`.

**Dry run**: *Actions > Release > Run workflow* on any branch builds everything and keeps the installers as workflow artifacts
(14 days) without publishing anything. Do this after changing the workflow, `tauri.conf.json` or the build profile.

The update check (`as_core::updates::select_asset`) picks the installer by the `<os>-<arch>` label and the extension (`.AppImage`,
`.dmg`, `.msi`) in the asset name: keep the naming. Linux arm64 is not built (add a `ubuntu-24.04-arm` row to the matrix when wanted).

## Not automated yet (future work)

Hydra updates Homebrew, AUR and winget from its release workflow (`hydra/.github/workflows/update-*.yml`, `hydra/packaging/`). Agent
Studio does not, for now. When it makes sense:

- **Homebrew**: a tap (`kawashita86/homebrew-tap`) with a cask that downloads the dmg and checks the SHA256 from `SHA256SUMS`.
- **winget / Scoop / Chocolatey**: manifests that point at the Windows `.msi`.
- **AUR**: a `agent-studio-bin` PKGBUILD over the Linux `.deb`/AppImage.
- Code signing / notarization of the macOS and Windows bundles (not done: Gatekeeper and SmartScreen warn on downloaded installers), and the Tauri updater plugin.
