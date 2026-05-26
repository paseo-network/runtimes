# Contributing

This repository carries the Paseo runtime definitions and supporting tooling.
Keep changes narrow and easy to review.

## Before Opening A PR

- State which runtime, chain spec, script, or monitoring component changed.
- Include the command you ran to test the change.
- Call out generated files such as weights or chain specs.
- Keep prose updates concise and tied to files in the repo.

## Runtime Changes

- Prefer small commits that separate runtime logic, generated output, and docs.
- Do not mix unrelated network changes in one PR.
- When porting from Polkadot SDK, note the upstream version or commit.

## Monitoring Changes

- Explain the signal, threshold, and alert route.
- Prefer runtime-derived thresholds over hardcoded timing assumptions.
- Keep dashboards and alerts aligned with the same Prometheus series.
