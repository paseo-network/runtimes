default: fmt

# Format code and toml files
fmt:
	#!/usr/bin/env bash

	cargo +nightly-2024-09-11 fmt --all
	taplo format --check --config .config/taplo.toml
