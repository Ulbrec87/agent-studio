# Agent Studio

**Run Claude Code, OpenAI Codex and OpenCode agents in parallel, from your terminal.**

Agent Studio is a terminal UI (Rust, ratatui) that spins up several AI coding CLI agents side by side,
each in its own pseudo-terminal, grouped by project. A background daemon owns the agents, so they keep
working when you close the UI — reattach any time, like tmux.

> Status: early development — porting the [Hydra](https://github.com/jpdlr/hydra) desktop app to a TUI.
> See [`docs/plans/2026-11-04-porting-plan.md`](docs/plans/2026-11-04-porting-plan.md).

## Planned features

- Chat, Grid and Overview views ("which agents need me?")
- Resume Claude / Codex sessions, multiple Claude profiles, git worktree per agent
- MCP manager agent that orchestrates the others, headless background runs
- Usage & plan-limits tracking, git panel, file browser, plain shells per project

## Build

```bash
cargo build --release
./target/release/agent-studio
```

Requires the CLIs you want to use on `PATH`: `claude`, `codex`, `opencode`.

## Credits

- [Hydra](https://github.com/jpdlr/hydra) by jpdlr (MIT) — the original desktop app this project ports.
- [agent-lookout](https://github.com/Olanetsoft/agent-lookout) by Idris Olubisi (MIT) — model for the Overview.

## License

[MIT](LICENSE)
