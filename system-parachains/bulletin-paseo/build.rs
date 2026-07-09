// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(all(not(feature = "metadata-hash"), feature = "std"))]
fn main() {
	substrate_wasm_builder::WasmBuilder::build_using_defaults();

	substrate_wasm_builder::WasmBuilder::init_with_defaults()
		.set_file_name("fast_runtime_binary.rs")
		.enable_feature("fast-runtime")
		.build();
}

#[cfg(all(feature = "metadata-hash", feature = "std"))]
fn main() {
	substrate_wasm_builder::WasmBuilder::init_with_defaults()
		.enable_metadata_hash("PAS", 10)
		.build();

	substrate_wasm_builder::WasmBuilder::init_with_defaults()
		.set_file_name("fast_runtime_binary.rs")
		.enable_feature("fast-runtime")
		.enable_metadata_hash("PAS", 10)
		.build();
}

#[cfg(not(feature = "std"))]
fn main() {}
