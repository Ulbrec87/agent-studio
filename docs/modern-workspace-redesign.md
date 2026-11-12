# Modern Workspace redesign

## Implementation boundaries

The redesign uses a single workspace layout with four color themes: Light, Dark Terracotta, Midnight (the default), and Modern Workspace Soft. The three approved reference images guide proportions and styling; existing controls and metric meanings take precedence over illustrative mockup content.

The renderer owns the Soft theme preference in webview storage. The daemon's theme enum, configuration shape, generated bindings, PTYs, and Rust code remain unchanged. Existing themes continue to use daemon configuration.

## Audit

- React 18, TypeScript and CSS Modules; shared CSS tokens supply the existing three palettes.
- `App` owns navigation and panel state. The sidebar currently appears only in Chat; the redesign shares it across all three views.
- Grid uses dnd-kit with persisted ordering, project selection, Running filtering and column choice. Its terminal instances and action callbacks must retain their identities during visual updates.
- Chat owns its editor split, composer, additional terminals and agent toolbar. Git retains its existing split/overlay placement, rather than adding the mockup's vertical editor/Git dock.
- Overview already provides counts, usage, Needs You, sessions/details, Last Hour, Events, Timeline and Waits. Existing scopes and periods must remain explicit; mockup statistics are not new data requirements.
- xterm observes CSS theme changes independently of its mount lifecycle. Fit scheduling waits for a usable box, forces the PTY size when visible again, and replays saved output only after the first fit. Preserve these protections.
- Preserve native titlebar drag regions, macOS traffic-light spacing, keyboard shortcuts and existing minimum window size (800 × 600).

## Baseline and validation

Node 22 dependencies were installed using the existing lockfile. Baseline: 629 tests across 70 files pass, typecheck passes, and lint has three existing warnings (unused `REASONING`, unused `NotificationType`, and an unused eslint-disable in `PierreFileTree`).

Frontend validation after the redesign:

- Typecheck: passed.
- Lint: passed with the same three pre-existing warnings, no new warnings or errors.
- Tests: 639 passed across 72 files. Added coverage includes renderer-only theme persistence, failure/race handling, shared sidebar navigation, stable Chat terminal identity when toggling the editor, and updating xterm colors without recreation.
- Production build: passed. No dependency or lockfile changes.
- `git diff --check`: passed.
- Rust, shared protocol types and generated IPC bindings: no changes.
- Soft palette contrast was calculated for normal text, muted text, selected controls and action buttons; colors were adjusted where needed. This is a token-level calculation, not a complete visual accessibility audit.

The browser inventory exposed no usable browser surface. Opening the integrated browser first timed out during automatic permission review; the retry returned `Browser is not available: iab`. Consequently no real application screenshots or visual comparisons were captured. The implementation must remain a draft until visual and native checks are completed; unit tests do not establish pixel fidelity or real PTY behavior.

The host lacks Rust and WebKit/GTK development headers. A dedicated Debian 12 Docker builder now supplies those build prerequisites without installing tools on the host. The resulting Linux release binary was exercised on the host through the native dev bridge: daemon ping, empty isolated profile, a real PTY command, resize to 100 × 32 and clean shutdown all passed. Interactive Tauri window behavior and visual fidelity still require a desktop check; the existing CI covers Linux, macOS and Windows.

## Desktop acceptance checklist

The following manual checks remain open:

- Compare Grid, Chat and Overview with the approved mockups at their reference sizes, at 1200 × 800, and at the minimum 800 × 600. Check all four palettes for overflow, focus visibility and readable disabled/hover states.
- With a real agent, type into xterm, switch Chat → Grid → Overview → Chat, change agents/projects, resize the window/sidebar/editor, and confirm output, cursor position and PTY dimensions remain correct.
- Exercise Auto/2/3 columns, tile reordering (pointer and keyboard), expansion, Running filtering, agent actions and broadcast.
- Exercise composer images, history, slash/model/branch menus, Monaco tabs/save, additional terminals and Git/diff operations in split and overlay modes.
- Check native window dragging and macOS traffic lights, platform shortcuts, keyboard-only navigation and reduced motion.
- Check the existing Linux/macOS/Windows CI results before making the PR ready for review.

## Intentional differences from the references

- Keep existing metrics, periods, menu actions and panel modes; do not add illustrative controls or synthetic statistics.
- Git continues using its existing split/overlay placements.
- The renewed layout is shared by every color theme. No legacy layout switch is maintained.
- Keep Midnight as the default color theme; Soft is opt-in.


## Local Linux preview

A follow-up palette revision reduces blue saturation in large surfaces, uses neutral anthracite/slate tones sampled from the approved references, and reserves blue for selected controls and accents. Header and sidebar use separate surface treatments; Chat toolbar controls use compact rounded rectangles. Monaco follows the revised palette. Existing themes retain their palettes.

The local Linux x86_64 build of commit `679faf83186a94482398552be0d5aac20aa7471a` is in the ignored `target/local-release/` directory:

- `run-preview.sh`: starts the real desktop app with the regular Agent Studio data directory, projects, sessions and settings.
- `run-isolated-preview.sh`: optional separate data directory for an independent trial. Automated smoke checks use an isolated temporary profile.
- `agent-studio`: stripped release executable.
- `agent-studio_0.1.2_amd64.deb`: optional installable Debian/Ubuntu/Mint package.
- `agent-studio-modern-workspace-linux-x64.tar.gz`: executable and launchers for running without installation.
- `SHA256SUMS`, `build-info.json`, and `native-smoke.json`: integrity and build/check metadata.

Build validation: frontend production build passed; 18 targeted appearance/Chat/xterm tests passed; lint retains only its three existing warnings; Rust formatting passed; native release and Debian packaging passed; host dynamic linking reports no missing libraries; native daemon/PTY smoke passed. The package retains version 0.1.2 and no GitHub release or tag was created.
