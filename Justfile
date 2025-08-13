default: fmt

# Format code and toml files
fmt:
	#!/usr/bin/env bash

	cargo +nightly-2025-01-30 fmt --all
	taplo format --check --config .config/taplo.toml
