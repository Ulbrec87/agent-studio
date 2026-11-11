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

Node 22 dependencies were installed using the existing lockfile. Baseline typecheck passes; lint has three existing warnings (unused `REASONING`, unused `NotificationType`, and an unused eslint-disable in `PierreFileTree`). Full test results and final validation are recorded below as work completes.

The current environment lacks Rust and WebKit/GTK development libraries. Native Tauri/PTY verification requires a suitable desktop environment; the existing CI checks Rust on Linux, macOS and Windows. Browser mock previews are not evidence of a real agent process.

## Intentional differences from the references

- Keep existing metrics, periods, menu actions and panel modes; do not add illustrative controls or synthetic statistics.
- Git continues using its existing split/overlay placements.
- The renewed layout is shared by every color theme. No legacy layout switch is maintained.
- Keep Midnight as the default color theme; Soft is opt-in.
