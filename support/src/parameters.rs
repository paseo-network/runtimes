// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0

//! Reusable adapters for runtime-configured parameters.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::marker::PhantomData;
use frame_support::traits::Get;
use scale_info::TypeInfo;
use sp_statement_store::StatementAllowance;

/// A [`StatementAllowance`] representation with a fixed maximum encoded length.
#[derive(
	Clone,
	Default,
	PartialEq,
	Eq,
	Debug,
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
)]
pub struct StatementAllowanceParameter {
	/// Maximum total statement size in bytes.
	pub max_size: u32,
	/// Maximum number of statements.
	pub max_count: u32,
}

impl From<StatementAllowanceParameter> for StatementAllowance {
	fn from(value: StatementAllowanceParameter) -> Self {
		Self { max_size: value.max_size, max_count: value.max_count }
	}
}

/// Adapts a [`StatementAllowanceParameter`] getter to the SDK allowance type.
pub struct StatementAllowanceGetter<Parameter>(PhantomData<Parameter>);
impl<Parameter: Get<StatementAllowanceParameter>> Get<StatementAllowance>
	for StatementAllowanceGetter<Parameter>
{
	fn get() -> StatementAllowance {
		Parameter::get().into()
	}
}

/// Clamps a `u32` or `u8` getter to at least one.
pub struct AtLeastOne<Inner>(PhantomData<Inner>);
impl<Inner: Get<u32>> Get<u32> for AtLeastOne<Inner> {
	fn get() -> u32 {
		Inner::get().max(1)
	}
}

impl<Inner: Get<u8>> Get<u8> for AtLeastOne<Inner> {
	fn get() -> u8 {
		Inner::get().max(1)
	}
}

/// Clamps the inner value to at most the value supplied by `Maximum`.
pub struct AtMost<Inner, Maximum>(PhantomData<(Inner, Maximum)>);
impl<Value: Ord, Inner: Get<Value>, Maximum: Get<Value>> Get<Value> for AtMost<Inner, Maximum> {
	fn get() -> Value {
		Inner::get().min(Maximum::get())
	}
}

/// Clamps the inner value to at least the value supplied by `Minimum`.
pub struct AtLeast<Inner, Minimum>(PhantomData<(Inner, Minimum)>);
impl<Value: Ord, Inner: Get<Value>, Minimum: Get<Value>> Get<Value> for AtLeast<Inner, Minimum> {
	fn get() -> Value {
		Inner::get().max(Minimum::get())
	}
}

/// Subtracts one with saturation from a `u32` getter.
pub struct SaturatingSubOne<Inner>(PhantomData<Inner>);
impl<Inner: Get<u32>> Get<u32> for SaturatingSubOne<Inner> {
	fn get() -> u32 {
		Inner::get().saturating_sub(1)
	}
}

/// Returns the inner value, or `Maximum` in `runtime-benchmarks` builds.
///
/// Benchmark sweeps and worst-case budget checks must cover the largest value reachable in
/// production, so benchmark builds pin the value to its maximum. The inner getter is still
/// evaluated so generated weights include the same storage reads production performs.
pub struct BenchmarkMax<Inner, Maximum>(PhantomData<(Inner, Maximum)>);
impl<Value, Inner: Get<Value>, Maximum: Get<Value>> Get<Value> for BenchmarkMax<Inner, Maximum> {
	fn get() -> Value {
		#[cfg(feature = "runtime-benchmarks")]
		{
			let _ = Inner::get();
			Maximum::get()
		}

		#[cfg(not(feature = "runtime-benchmarks"))]
		{
			Inner::get()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use core::sync::atomic::{AtomicU32, Ordering};
	use frame_support::traits::{ConstU32, ConstU8};

	struct ZeroU32;
	impl Get<u32> for ZeroU32 {
		fn get() -> u32 {
			0
		}
	}

	struct ZeroU8;
	impl Get<u8> for ZeroU8 {
		fn get() -> u8 {
			0
		}
	}

	struct Allowance;
	impl Get<StatementAllowanceParameter> for Allowance {
		fn get() -> StatementAllowanceParameter {
			StatementAllowanceParameter { max_size: 42, max_count: 3 }
		}
	}

	#[test]
	fn statement_allowance_getter_adapts_the_value() {
		assert_eq!(
			StatementAllowanceGetter::<Allowance>::get(),
			StatementAllowance { max_size: 42, max_count: 3 }
		);
	}

	#[test]
	fn at_least_one_raises_zero_to_one_and_passes_positive_values_through() {
		assert_eq!(AtLeastOne::<ZeroU32>::get(), 1);
		assert_eq!(AtLeastOne::<ZeroU8>::get(), 1);
		assert_eq!(AtLeastOne::<ConstU32<5>>::get(), 5);
		assert_eq!(AtLeastOne::<ConstU8<5>>::get(), 5);
	}

	#[test]
	fn at_most_preserves_in_range_values_and_clamps_the_maximum() {
		assert_eq!(<AtMost<ConstU32<10>, ConstU32<20>> as Get<u32>>::get(), 10);
		assert_eq!(<AtMost<ConstU32<30>, ConstU32<20>> as Get<u32>>::get(), 20);
	}

	#[test]
	fn at_least_preserves_in_range_values_and_clamps_the_minimum() {
		assert_eq!(<AtLeast<ConstU32<20>, ConstU32<10>> as Get<u32>>::get(), 20);
		assert_eq!(<AtLeast<ConstU32<10>, ConstU32<20>> as Get<u32>>::get(), 20);
	}

	#[test]
	fn saturating_sub_one_never_underflows() {
		assert_eq!(SaturatingSubOne::<ConstU32<0>>::get(), 0);
		assert_eq!(SaturatingSubOne::<ConstU32<10>>::get(), 9);
	}

	#[cfg(not(feature = "runtime-benchmarks"))]
	#[test]
	fn benchmark_max_returns_the_inner_value_outside_benchmark_builds() {
		assert_eq!(<BenchmarkMax<ConstU32<10>, ConstU32<20>> as Get<u32>>::get(), 10);
	}

	#[cfg(feature = "runtime-benchmarks")]
	#[test]
	fn benchmark_max_returns_the_maximum_in_benchmark_builds() {
		assert_eq!(<BenchmarkMax<ConstU32<10>, ConstU32<20>> as Get<u32>>::get(), 20);
	}

	/// Weights are generated in benchmark builds, so if the inner getter were skipped there, the
	/// storage read it performs in production would be missing from the generated weights.
	#[test]
	fn benchmark_max_reads_the_inner_value_in_every_build_mode() {
		static INNER_READS: AtomicU32 = AtomicU32::new(0);

		struct CountingInner;
		impl Get<u32> for CountingInner {
			fn get() -> u32 {
				INNER_READS.fetch_add(1, Ordering::Relaxed);
				10
			}
		}

		let _ = <BenchmarkMax<CountingInner, ConstU32<20>> as Get<u32>>::get();
		assert_eq!(INNER_READS.load(Ordering::Relaxed), 1);
	}
}
