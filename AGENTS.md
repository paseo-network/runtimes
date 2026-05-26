# Agent Notes

Use this file when working on the repo with coding agents.

## Ground Rules

- Treat the Rust runtime code, chain specs, and scripts as the source of truth.
- Keep documentation short. Explain the intent, not every implementation detail.
- Do not edit generated weights unless the task is explicitly about weights.
- Do not rewrite unrelated files while making a focused runtime or tooling change.
- Preserve existing formatting and naming unless there is a clear reason to change it.

## Useful Checks

- Run `cargo fmt` after Rust edits.
- Run the smallest relevant test first. Broaden only when the change justifies it.
- For docs-only changes, check links, commands, and file paths.

## Monitoring Work

- Keep monitoring thresholds tied to runtime constants where possible.
- Document alert behavior from the operator's point of view.
- Avoid duplicating exporter logic in prose. Point to the code when exact behavior matters.
