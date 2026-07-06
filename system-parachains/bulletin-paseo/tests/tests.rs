// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(test)]

use bulletin_paseo_runtime as runtime;
use bulletin_paseo_runtime::{
	paseo_constants::fee::WeightToFee, xcm_config::LocationToAccountId, AllPalletsWithoutSystem,
	Balances, Block, HopPromotion, Runtime, RuntimeCall, RuntimeEvent, RuntimeGenesisConfig,
	RuntimeOrigin, SessionKeys, System, TransactionStorage, TxExtension, UncheckedExtrinsic,
};
use bulletin_transaction_storage_primitives::cids::{calculate_cid, CidConfig, HashingAlgorithm};
use frame_support::{
	assert_err, assert_ok, dispatch::GetDispatchInfo, pallet_prelude::Hooks, traits::Get,
};
use pallet_bulletin_transaction_storage::{
	extension::{AllowanceBasedPriority, ALLOWANCE_PRIORITY_BOOST},
	AuthorizationExtent, AuthorizationScope, Call as TxStorageCall, Config as TxStorageConfig,
	Origin as TxStorageOrigin, TransactionRef,
};
use parachains_common::{AccountId, AuraId, BlockNumber, Hash as PcHash, Signature as PcSignature};
use parachains_runtimes_test_utils::{ExtBuilder, GovernanceOrigin, RuntimeHelper};
use sp_core::{crypto::Ss58Codec, Encode, Pair};
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
	traits::{TransactionExtension, TxBaseImplication},
	transaction_validity,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
	ApplyExtrinsicResult, BuildStorage, DispatchError, Either,
};
use std::collections::HashMap;
use xcm::latest::prelude::*;
use xcm_runtime_apis::conversions::LocationToAccountHelper;

const ALICE: [u8; 32] = [1u8; 32];

/// Governance location used in tests: paraId 1500, a configured `GovernanceParachainIds` member.
fn governance_location() -> Location {
	Location::new(1, [Parachain(1500)])
}

/// Advance to the next block for testing transaction storage.
fn advance_block() {
	let current = frame_system::Pallet::<Runtime>::block_number();

	<TransactionStorage as Hooks<_>>::on_finalize(current);
	<System as Hooks<_>>::on_finalize(current);

	let next = current + 1;
	System::set_block_number(next);

	frame_system::BlockWeight::<Runtime>::kill();
	frame_system::BlockSize::<Runtime>::kill();

	<System as Hooks<_>>::on_initialize(next);
	<TransactionStorage as Hooks<_>>::on_initialize(next);
}

fn construct_extrinsic(
	sender: Option<sp_core::sr25519::Pair>,
	call: RuntimeCall,
) -> Result<UncheckedExtrinsic, transaction_validity::TransactionValidityError> {
	// provide a known block hash for the immortal era check
	frame_system::BlockHash::<Runtime>::insert(0, PcHash::default());
	let inner = (
		frame_system::AuthorizeCall::<Runtime>::new(),
		frame_system::CheckNonZeroSender::<Runtime>::new(),
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckEra::<Runtime>::from(sp_runtime::generic::Era::immortal()),
		frame_system::CheckNonce::<Runtime>::from(if let Some(s) = sender.as_ref() {
			let account_id = AccountId::from(s.public());
			frame_system::Pallet::<Runtime>::account(&account_id).nonce
		} else {
			0
		}),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_skip_feeless_payment::SkipCheckIfFeeless::from(
			pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0u128),
		),
		pallet_bulletin_transaction_storage::extension::ValidateStorageCalls::<
			Runtime,
			bulletin_paseo_runtime::storage::StorageCallInspector,
		>::default(),
		pallet_bulletin_transaction_storage::extension::AllowanceBasedPriority::<Runtime>::default(
		),
		frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
	);
	let tx_ext: TxExtension =
		cumulus_pallet_weight_reclaim::StorageWeightReclaim::<Runtime, _>::from(inner);

	if let Some(s) = sender.as_ref() {
		// Signed call.
		let account_id = AccountId::from(s.public());
		let payload = sp_runtime::generic::SignedPayload::new(call.clone(), tx_ext.clone())?;
		let signature = payload.using_encoded(|e| s.sign(e));
		Ok(UncheckedExtrinsic::new_signed(
			call,
			account_id.into(),
			PcSignature::Sr25519(signature),
			tx_ext,
		))
	} else {
		// Unsigned call.
		Ok(UncheckedExtrinsic::new_transaction(call, tx_ext))
	}
}

fn construct_and_apply_extrinsic(
	account: Option<sp_core::sr25519::Pair>,
	call: RuntimeCall,
) -> ApplyExtrinsicResult {
	let dispatch_info = call.get_dispatch_info();
	let xt = construct_extrinsic(account, call)?;
	let xt_len = xt.encode().len();
	tracing::info!(
		"Applying extrinsic: class={:?} pays_fee={:?} weight={:?} encoded_len={} bytes",
		dispatch_info.class,
		dispatch_info.pays_fee,
		dispatch_info.total_weight(),
		xt_len
	);
	bulletin_paseo_runtime::Executive::apply_extrinsic(xt)
}

fn assert_ok_ok(apply_result: ApplyExtrinsicResult) {
	assert_ok!(apply_result);
	assert_ok!(apply_result.unwrap());
}

#[test]
fn transaction_storage_runtime_sizes() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			// prepare data
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let max =
				<<Runtime as TxStorageConfig>::MaxTransactionSize as Get<u32>>::get() as usize;
			let sizes: [usize; 6] = [
				1,           // minimum valid size
				2000,        // small
				max / 4,     // 25%
				max / 2,     // 50%
				max * 3 / 4, // 75%
				max,         // 100% (exactly at limit)
			];
			let total_bytes: u64 = sizes.iter().map(|s| *s as u64).sum();

			// authorize
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				total_bytes
			));
			assert_eq!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: total_bytes,
					transactions: 0,
					transactions_allowance: 0,
				},
			);

			// store data via signed extrinsics (ValidateSigned consumes authorization)
			for (index, size) in sizes.into_iter().enumerate() {
				// Advance to a new block for each store
				advance_block();

				tracing::info!("Storing data with size: {size} and index: {index}");
				let res = construct_and_apply_extrinsic(
					Some(account.pair()),
					RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
						data: vec![0u8; size],
					}),
				);
				assert_ok!(res);
				assert_ok!(res.unwrap());
			}
			assert_eq!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: total_bytes,
					bytes_permanent: 0,
					bytes_allowance: total_bytes,
					transactions: 6,
					transactions_allowance: 0,
				},
			);

			// (MaxTransactionSize+1) should exceed MaxTransactionSize and fail
			let oversized: u64 =
				(<<Runtime as TxStorageConfig>::MaxTransactionSize as frame_support::traits::Get<
					u32,
				>>::get() + 1)
					.into();
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				oversized
			));
			// Re-authorize within the unexpired window adds to the existing allowance;
			// `bytes` (used) is preserved and expiry is unchanged.
			assert_eq!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: total_bytes,
					bytes_permanent: 0,
					bytes_allowance: total_bytes + oversized,
					transactions: 6,
					transactions_allowance: 0,
				},
			);
			let res = construct_and_apply_extrinsic(
				Some(account.pair()),
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
					data: vec![0u8; oversized as usize],
				}),
			);
			// On the Westend, very large extrinsics may be rejected earlier for exhausting
			// resources (block length/weight) before reaching the pallet's BAD_DATA_SIZE check.
			assert!(
				res == Err(pallet_bulletin_transaction_storage::BAD_DATA_SIZE.into()) ||
					res == Err(InvalidTransaction::ExhaustsResources.into()),
				"unexpected error: {res:?}"
			);
		});
}

/// Test maximum write throughput: 8 transactions of 1 MiB each in a single block (8 MiB total).
#[test]
fn transaction_storage_max_throughput_per_block() {
	const NUM_TRANSACTIONS: u32 = 8;
	const TRANSACTION_SIZE: u64 = 1024 * 1024; // 1 MiB

	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();

			// authorize 8+1 transactions of 1 MiB each
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				(NUM_TRANSACTIONS as u64 + 1) * TRANSACTION_SIZE
			));
			assert_eq!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: (NUM_TRANSACTIONS as u64 + 1) * TRANSACTION_SIZE,
					transactions: 0,
					transactions_allowance: 0,
				},
			);

			// Advance to a fresh block
			advance_block();

			// Store all 8 transactions in the same block (no advance_block between them)
			for index in 0..NUM_TRANSACTIONS {
				let res = construct_and_apply_extrinsic(
					Some(account.pair()),
					RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
						data: vec![index as u8; TRANSACTION_SIZE as _],
					}),
				);
				assert_ok!(res);
				assert_ok!(res.unwrap());
			}

			// 9th should fail.
			assert_err!(
				construct_and_apply_extrinsic(
					Some(account.pair()),
					RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
						data: vec![0u8; TRANSACTION_SIZE as _],
					}),
				),
				transaction_validity::TransactionValidityError::Invalid(
					InvalidTransaction::ExhaustsResources
				)
			);

			// Verify just 8 authorizations were consumed
			assert_eq!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: NUM_TRANSACTIONS as u64 * TRANSACTION_SIZE,
					bytes_permanent: 0,
					bytes_allowance: (NUM_TRANSACTIONS as u64 + 1) * TRANSACTION_SIZE,
					transactions: NUM_TRANSACTIONS,
					transactions_allowance: 0,
				},
			);
		});
}

#[test]
fn authorized_storage_transactions_are_for_free() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			// 1. user authorization flow.
			let account = Sr25519Keyring::Eve;
			let who: AccountId = account.to_account_id();
			let data = vec![0u8; 24];
			let content_hash = sp_io::hashing::blake2_256(&data);
			let call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});

			// Not authorized account should fail to store.
			assert_err!(
				construct_and_apply_extrinsic(Some(account.pair()), call.clone()),
				transaction_validity::TransactionValidityError::Invalid(
					InvalidTransaction::Payment
				)
			);
			// Authorize user for store + renew + enable_auto_renew.
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				48
			));
			// Store should now work without funding (feeless).
			let stored_block = System::block_number();
			let res = construct_and_apply_extrinsic(Some(account.pair()), call);
			assert_ok!(res);
			assert_ok!(res.unwrap());

			advance_block();

			// Renew should also work without funding (feeless).
			let renew_call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::force_renew {
					entry: TransactionRef::Position { block: stored_block, index: 0 },
				});
			let res = construct_and_apply_extrinsic(Some(account.pair()), renew_call);
			assert_ok!(res);
			assert_ok!(res.unwrap());

			advance_block();

			// `enable_auto_renew` is feeless but pre-pays one cycle at registration
			// — same hard-cap accounting as `force_renew`. The next cycle fires
			// free thanks to `paid: true`; subsequent cycles charge per-cycle.
			let extent_before = TransactionStorage::account_authorization_extent(who.clone());
			let enable_call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::enable_auto_renew {
					content_hash,
				});
			let res = construct_and_apply_extrinsic(Some(account.pair()), enable_call);
			assert_ok!(res);
			assert_ok!(res.unwrap());
			let extent_after = TransactionStorage::account_authorization_extent(who.clone());
			assert_eq!(extent_after.transactions, extent_before.transactions + 1);
			assert_eq!(extent_after.bytes, extent_before.bytes);
			assert_eq!(
				extent_after.bytes_permanent,
				extent_before.bytes_permanent + data.len() as u64,
			);
		});
}

/// One-shot `renew` pre-pays the renewal at registration time. Verify that
/// `bytes_permanent` advances by `info.size` at the runtime level.
#[test]
fn renew_one_shot_prepays_bytes_permanent() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			let account = Sr25519Keyring::Bob;
			let who: AccountId = account.to_account_id();
			let data = vec![0u8; 24];
			let content_hash = sp_io::hashing::blake2_256(&data);

			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				2,
				48,
			));
			assert_ok!(construct_and_apply_extrinsic(
				Some(account.pair()),
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
					data: data.clone(),
				}),
			)
			.unwrap());
			advance_block();

			let before = TransactionStorage::account_authorization_extent(who.clone());
			let renew_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::renew {
				entry: TransactionRef::ContentHash(content_hash),
			});
			let res = construct_and_apply_extrinsic(Some(account.pair()), renew_call);
			assert_ok!(res);
			assert_ok!(res.unwrap());

			let after = TransactionStorage::account_authorization_extent(who);
			assert_eq!(after.bytes_permanent, before.bytes_permanent + data.len() as u64);
			assert_eq!(after.transactions, before.transactions + 1);
		});
}

/// Run `AllowanceBasedPriority::validate` and return the contributed priority.
fn allowance_based_priority(
	origin: RuntimeOrigin,
	call: &RuntimeCall,
) -> transaction_validity::TransactionPriority {
	let info = call.get_dispatch_info();
	AllowanceBasedPriority::<Runtime>::default()
		.validate(origin, call, &info, 0, (), &TxBaseImplication(()), TransactionSource::External)
		.expect("validate should not fail")
		.0
		.priority
}

#[test]
fn allowance_based_priority_works() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			let account = Sr25519Keyring::Eve;
			let who: AccountId = account.to_account_id();
			let origin: RuntimeOrigin = TxStorageOrigin::<Runtime>::Authorized {
				who: who.clone(),
				scope: AuthorizationScope::Account(who.clone()),
			}
			.into();
			let store = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: vec![0u8; 1],
			});

			// No authorization → no boost.
			assert_eq!(allowance_based_priority(origin.clone(), &store), 0);

			// In-budget → flat boost (the strategy unit-tests live in the pallet; this
			// integration test just confirms the wired-up extension reports a boost while
			// the signer is authorized).
			let allowance: u64 = 4_000;
			let tx_allowance: u32 = 10;
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				tx_allowance,
				allowance,
			));
			assert_eq!(allowance_based_priority(origin.clone(), &store), ALLOWANCE_PRIORITY_BOOST);

			// Eager-compute: a single tx whose size alone would push the signer over the cap
			// is demoted to `0` boost on entry, even though current `bytes == 0`.
			let oversized_store =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
					data: vec![0u8; allowance as usize + 1],
				});
			assert_eq!(allowance_based_priority(origin.clone(), &oversized_store), 0);

			// Consume the entire allowance → over-budget → no boost.
			assert_ok_ok(construct_and_apply_extrinsic(
				Some(account.pair()),
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
					data: vec![0u8; allowance as usize],
				}),
			));
			advance_block();
			assert_eq!(allowance_based_priority(origin.clone(), &store), 0);

			// `renew` carries `Origin::Authorized` too, but must not be boosted.
			let renew = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::force_renew {
				entry: TransactionRef::Position { block: 1, index: 0 },
			});
			assert_eq!(allowance_based_priority(origin, &renew), 0);
		});
}

#[test]
fn store_with_cid_config_works() {
	ExtBuilder::<Runtime>::default().with_tracing().build().execute_with(|| {
		// prepare data
		let account = Sr25519Keyring::Alice;
		let who: AccountId = account.to_account_id();
		let data = vec![0u8; 4 * 1024];
		let total_bytes: u64 = data.len() as u64;
		let block_number = System::block_number();

		// Authorize.
		assert_ok!(runtime::TransactionStorage::authorize_account(
			RuntimeOrigin::root(),
			who.clone(),
			0,
			3 * total_bytes
		));
		assert_eq!(
			runtime::TransactionStorage::account_authorization_extent(who.clone()),
			AuthorizationExtent {
				bytes: 0,
				bytes_permanent: 0,
				bytes_allowance: 3 * total_bytes,
				transactions: 0,
				transactions_allowance: 0,
			},
		);

		// 1. Store data WITHOUT a custom cid_config (plain `store`).
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(account.pair()),
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data: data.clone() }),
		));

		// 2. Store data WITH a cid_config as the default codec for raw data via
		//    `store_with_cid_config`.
		// (Should produce the same content_hash as above).
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(account.pair()),
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store_with_cid_config {
				cid: CidConfig { codec: 0x55, hashing: HashingAlgorithm::Blake2b256 },
				data: data.clone(),
			}),
		));

		// 3. Store data WITH a custom cid_config (Sha2_256 + 0x70 codec) via
		//    `store_with_cid_config`.
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(account.pair()),
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store_with_cid_config {
				cid: CidConfig { codec: 0x70, hashing: HashingAlgorithm::Sha2_256 },
				data: data.clone(),
			}),
		));

		// Check the content_hashes and CIDs.
		runtime::TransactionStorage::on_finalize(block_number);
		let stored_txs = runtime::TransactionStorage::transaction_roots(block_number)
			.unwrap()
			.into_iter()
			.enumerate()
			.collect::<HashMap<_, _>>();
		assert_eq!(stored_txs.len(), 3);
		assert_eq!(
			stored_txs[&0].content_hash,
			calculate_cid(&data, CidConfig { codec: 0x55, hashing: HashingAlgorithm::Blake2b256 })
				.unwrap()
				.content_hash
		);
		assert_eq!(stored_txs[&0].content_hash, stored_txs[&1].content_hash);
		assert_ne!(stored_txs[&0].content_hash, stored_txs[&2].content_hash);
	});
}

mod hop_tests {
	use super::*;

	#[test]
	fn is_promoted_on_chain_works() {
		sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
			.execute_with(|| {
				let account = Sr25519Keyring::Alice;
				let who: AccountId = account.to_account_id();
				let data = b"some-promoted-blob".to_vec();
				let content_hash = sp_io::hashing::blake2_256(&data);

				// Nothing stored yet — unknown hash returns false.
				assert!(!HopPromotion::is_promoted_on_chain(content_hash));

				// Authorize Alice and store the blob via `TransactionStorage::store`. Default
				// hashing is `Blake2b256`, which matches `content_hash` above.
				assert_ok!(TransactionStorage::authorize_account(
					RuntimeOrigin::root(),
					who.clone(),
					0,
					data.len() as u64,
				));
				advance_block();
				assert_ok_ok(construct_and_apply_extrinsic(
					Some(account.pair()),
					RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
						data: data.clone(),
					}),
				));
				// `BlockTransactions` is moved into `Transactions[block]` in `on_finalize`.
				advance_block();

				// Stored hash is now visible; an unrelated hash is not.
				assert!(HopPromotion::is_promoted_on_chain(content_hash));
				assert!(!HopPromotion::is_promoted_on_chain([0xAB; 32]));
			});
	}

	#[test]
	fn create_promotion_extrinsic_works() {
		use bulletin_paseo_runtime::Executive;
		use frame_system::offchain::CreateAuthorizedTransaction;
		use sp_io::hashing::blake2_256;
		use sp_runtime::{traits::IdentifyAccount, MultiSignature, MultiSigner};

		sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
			.execute_with(|| {
				let alice = Sr25519Keyring::Alice;
				let signer = MultiSigner::Sr25519(alice.public());
				let who: AccountId = signer.clone().into_account();
				let data = b"hop-promoted-data".to_vec();
				let content_hash = blake2_256(&data);
				let submit_timestamp: u64 = 0;

				// Nothing stored yet — unknown hash returns false.
				assert!(!HopPromotion::is_promoted_on_chain(content_hash));

				// Authorize Alice's account so the promotion's authorize closure passes.
				assert_ok!(TransactionStorage::authorize_account(
					RuntimeOrigin::root(),
					who.clone(),
					0,
					data.len() as u64,
				));
				advance_block();

				// Build the promotion extrinsic via the same path the `create_promotion_extrinsic`
				// runtime API uses.
				let payload =
					pallet_bulletin_hop_promotion::signing_payload(&content_hash, submit_timestamp);
				let signature = MultiSignature::Sr25519(alice.pair().sign(&payload));
				let xt = <Runtime as CreateAuthorizedTransaction<
					pallet_bulletin_hop_promotion::Call<Runtime>,
				>>::create_authorized_transaction(
					pallet_bulletin_hop_promotion::Call::<Runtime>::promote {
						data: data.clone(),
						signer,
						signature,
						submit_timestamp,
					}
					.into(),
				);

				// Apply the extrinsic and confirm dispatch succeeded.
				assert_ok_ok(Executive::apply_extrinsic(xt));

				// After block flush, the promoted hash is visible on-chain.
				advance_block();
				assert!(HopPromotion::is_promoted_on_chain(content_hash));
			});
	}
}

#[test]
fn location_conversion_works() {
	// the purpose of hardcoded values is to catch an unintended location conversion logic change.
	struct TestCase {
		description: &'static str,
		location: Location,
		expected_account_id_str: &'static str,
	}

	let test_cases = vec![
		// DescribeTerminus
		TestCase {
			description: "DescribeTerminus Parent",
			location: Location::new(1, Here),
			expected_account_id_str: "5Dt6dpkWPwLaH4BBCKJwjiWrFVAGyYk3tLUabvyn4v7KtESG",
		},
		TestCase {
			description: "DescribeTerminus Sibling",
			location: Location::new(1, [Parachain(1111)]),
			expected_account_id_str: "5Eg2fnssmmJnF3z1iZ1NouAuzciDaaDQH7qURAy3w15jULDk",
		},
		// DescribePalletTerminal
		TestCase {
			description: "DescribePalletTerminal Parent",
			location: Location::new(1, [PalletInstance(50)]),
			expected_account_id_str: "5CnwemvaAXkWFVwibiCvf2EjqwiqBi29S5cLLydZLEaEw6jZ",
		},
		TestCase {
			description: "DescribePalletTerminal Sibling",
			location: Location::new(1, [Parachain(1111), PalletInstance(50)]),
			expected_account_id_str: "5GFBgPjpEQPdaxEnFirUoa51u5erVx84twYxJVuBRAT2UP2g",
		},
		// DescribeAccountId32Terminal
		TestCase {
			description: "DescribeAccountId32Terminal Parent",
			location: Location::new(
				1,
				[Junction::AccountId32 { network: None, id: AccountId::from(ALICE).into() }],
			),
			expected_account_id_str: "5DN5SGsuUG7PAqFL47J9meViwdnk9AdeSWKFkcHC45hEzVz4",
		},
		TestCase {
			description: "DescribeAccountId32Terminal Sibling",
			location: Location::new(
				1,
				[
					Parachain(1111),
					Junction::AccountId32 { network: None, id: AccountId::from(ALICE).into() },
				],
			),
			expected_account_id_str: "5DGRXLYwWGce7wvm14vX1Ms4Vf118FSWQbJkyQigY2pfm6bg",
		},
		// DescribeAccountKey20Terminal
		TestCase {
			description: "DescribeAccountKey20Terminal Parent",
			location: Location::new(1, [AccountKey20 { network: None, key: [0u8; 20] }]),
			expected_account_id_str: "5F5Ec11567pa919wJkX6VHtv2ZXS5W698YCW35EdEbrg14cg",
		},
		TestCase {
			description: "DescribeAccountKey20Terminal Sibling",
			location: Location::new(
				1,
				[Parachain(1111), AccountKey20 { network: None, key: [0u8; 20] }],
			),
			expected_account_id_str: "5CB2FbUds2qvcJNhDiTbRZwiS3trAy6ydFGMSVutmYijpPAg",
		},
		// DescribeTreasuryVoiceTerminal
		TestCase {
			description: "DescribeTreasuryVoiceTerminal Parent",
			location: Location::new(1, [Plurality { id: BodyId::Treasury, part: BodyPart::Voice }]),
			expected_account_id_str: "5CUjnE2vgcUCuhxPwFoQ5r7p1DkhujgvMNDHaF2bLqRp4D5F",
		},
		TestCase {
			description: "DescribeTreasuryVoiceTerminal Sibling",
			location: Location::new(
				1,
				[Parachain(1111), Plurality { id: BodyId::Treasury, part: BodyPart::Voice }],
			),
			expected_account_id_str: "5G6TDwaVgbWmhqRUKjBhRRnH4ry9L9cjRymUEmiRsLbSE4gB",
		},
		// DescribeBodyTerminal
		TestCase {
			description: "DescribeBodyTerminal Parent",
			location: Location::new(1, [Plurality { id: BodyId::Unit, part: BodyPart::Voice }]),
			expected_account_id_str: "5EBRMTBkDisEXsaN283SRbzx9Xf2PXwUxxFCJohSGo4jYe6B",
		},
		TestCase {
			description: "DescribeBodyTerminal Sibling",
			location: Location::new(
				1,
				[Parachain(1111), Plurality { id: BodyId::Unit, part: BodyPart::Voice }],
			),
			expected_account_id_str: "5DBoExvojy8tYnHgLL97phNH975CyT45PWTZEeGoBZfAyRMH",
		},
	];

	for tc in test_cases {
		let expected =
			AccountId::from_string(tc.expected_account_id_str).expect("Invalid AccountId string");

		let got = LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
			tc.location.into(),
		)
		.unwrap();

		assert_eq!(got, expected, "{}", tc.description);
	}
}

#[test]
fn xcm_payment_api_works() {
	parachains_runtimes_test_utils::test_cases::xcm_payment_api_with_native_token_works::<
		Runtime,
		RuntimeCall,
		RuntimeOrigin,
		Block,
		WeightToFee,
	>();
}

#[test]
fn governance_authorize_upgrade_works() {
	use bulletin_paseo_runtime::paseo_constants::system_parachain::{ASSET_HUB_ID, COLLECTIVES_ID};

	// no - random para: not in the governance/authorizer allowlists, so the Barrier
	// rejects its unpaid execution before the Transact origin check is even reached.
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(12334)))),
		Either::Right(InstructionError { index: 0, error: XcmError::Barrier })
	);
	// no - AssetHub: not in the governance/authorizer allowlists, so the Barrier
	// rejects its unpaid execution before the Transact origin check is even reached.
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(ASSET_HUB_ID)))),
		Either::Right(InstructionError { index: 0, error: XcmError::Barrier })
	);
	// no - Collectives: a bare Collectives parachain origin is not in the
	// governance/authorizer allowlists, so the Barrier rejects it.
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(COLLECTIVES_ID)))),
		Either::Right(InstructionError { index: 0, error: XcmError::Barrier })
	);
	// no - Collectives Voice of Fellows plurality
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::LocationAndDescendOrigin(
			Location::new(1, Parachain(COLLECTIVES_ID)),
			Plurality { id: BodyId::Technical, part: BodyPart::Voice }.into()
		)),
		Either::Right(InstructionError { index: 2, error: XcmError::BadOrigin })
	);

	// no - relaychain (relay chain does not have superuser access, only `GovernanceParachainIds`
	// members do)
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::parent())),
		Either::Right(InstructionError { index: 1, error: XcmError::BadOrigin })
	);

	// ok - governance location (paraId 1500)
	assert_ok!(parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
		Runtime,
		RuntimeOrigin,
	>(GovernanceOrigin::Location(governance_location())));
}

#[test]
fn authorize_account_fee_path_follows_feeless_flag() {
	// `feeless: false` → fee charged, unfunded caller rejected.
	// `feeless: true`  → fee skipped via `SkipCheckIfFeeless`, unfunded caller succeeds.
	let alice = Sr25519Keyring::Alice;
	let authorizer = Sr25519Keyring::Charlie;
	let call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
		who: Sr25519Keyring::Eve.to_account_id(),
		transactions: 0,
		bytes: 1024,
	});

	let attempt = |feeless: bool, funded: bool| {
		let mut genesis = RuntimeGenesisConfig::default();
		genesis.transaction_storage.allowed_authorizers = vec![];
		genesis.sudo.key = Some(alice.to_account_id());
		sp_io::TestExternalities::new(genesis.build_storage().unwrap()).execute_with(|| {
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&alice.to_account_id(), 1_000_000_000_000_000).unwrap();
			if funded {
				Balances::mint_into(&authorizer.to_account_id(), 1_000_000_000_000_000).unwrap();
			}
			let add = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::add_authorizer {
				who: authorizer.to_account_id(),
				budget: pallet_bulletin_transaction_storage::AuthorizerBudget {
					feeless,
					..default_authorizer_budget()
				},
			});
			assert_ok_ok(construct_and_apply_extrinsic(
				Some(alice.pair()),
				RuntimeCall::Sudo(pallet_sudo::Call::<Runtime>::sudo { call: Box::new(add) }),
			));
			construct_and_apply_extrinsic(Some(authorizer.pair()), call.clone())
		})
	};

	assert_ok_ok(attempt(false, true));
	assert_eq!(
		attempt(false, false),
		Err(transaction_validity::TransactionValidityError::Invalid(InvalidTransaction::Payment)),
	);
	assert_ok_ok(attempt(true, false));
}

#[test]
fn non_authorizer_cannot_sign_authorize_account_extrinsic() {
	// Eve is NOT a TestAccount/Authorizer. Her signed `authorize_account` extrinsic should
	// be rejected at validation with BadSigner (checked in pallet's check_signed).
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			let eve = Sr25519Keyring::Eve;
			let target = Sr25519Keyring::Ferdie;

			// Give Eve balance so the fee check (before ValidateSigned) passes.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&eve.to_account_id(), 1_000_000_000_000).unwrap();

			let call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
					who: target.to_account_id(),
					transactions: 0,
					bytes: 1024,
				});

			assert_eq!(
				construct_and_apply_extrinsic(Some(eve.pair()), call),
				Err(transaction_validity::TransactionValidityError::Invalid(
					InvalidTransaction::BadSigner
				)),
			);
		});
}

#[test]
fn people_chain_can_authorize_storage_with_transact() {
	// People chain (parachain 1502) should be able to authorize storage via XCM Transact.
	let people_location = Location::new(1, [Parachain(1502)]);

	let account = Sr25519Keyring::Ferdie;
	let authorize_call = RuntimeCall::TransactionStorage(
		pallet_bulletin_transaction_storage::Call::<Runtime>::authorize_account {
			who: account.to_account_id(),
			transactions: 0,
			bytes: 1024,
		},
	);

	// Execute XCM as People chain origin would do with `Transact -> Origin::Xcm`.
	ExtBuilder::<Runtime>::default()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.with_tracing()
		.build()
		.execute_with(|| {
			assert_ok!(RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
				(people_location, OriginKind::Xcm),
				authorize_call,
				None
			)
			.ensure_complete());

			// Check event.
			System::assert_has_event(RuntimeEvent::TransactionStorage(
				pallet_bulletin_transaction_storage::Event::AccountAuthorized {
					who: account.to_account_id(),
					transactions: 0,
					bytes: 1024,
				},
			));
		})
}

#[test]
fn people_next_chain_can_authorize_storage_with_transact() {
	// PeopleNext chain (parachain 5140) should be able to authorize storage via XCM Transact,
	// similar to the People chain.
	let people_next_location = Location::new(1, [Parachain(5140)]);

	let account = Sr25519Keyring::Ferdie;
	let authorize_call = RuntimeCall::TransactionStorage(
		pallet_bulletin_transaction_storage::Call::<Runtime>::authorize_account {
			who: account.to_account_id(),
			transactions: 0,
			bytes: 1024,
		},
	);

	ExtBuilder::<Runtime>::default()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.with_tracing()
		.build()
		.execute_with(|| {
			assert_ok!(RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
				(people_next_location, OriginKind::Xcm),
				authorize_call,
				None
			)
			.ensure_complete());

			// Check event.
			System::assert_has_event(RuntimeEvent::TransactionStorage(
				pallet_bulletin_transaction_storage::Event::AccountAuthorized {
					who: account.to_account_id(),
					transactions: 0,
					bytes: 1024,
				},
			));
		})
}

/// See [`pallet_bulletin_transaction_storage::ensure_weight_sanity`].
#[test]
fn transaction_storage_weight_sanity() {
	pallet_bulletin_transaction_storage::ensure_weight_sanity::<Runtime>(
		// Collator-side PoV cap: default 85% of max_pov_size.
		// See cumulus/client/consensus/aura/src/collators/slot_based/block_builder_task.rs
		Some(85),
	);
}

// ============================================================================
// Ensure calls wrapped in dispatch wrappers are subject to the same validation
// as direct submissions. Covers utility (batch, batch_all, force_batch,
// as_derivative) and sudo.
// ============================================================================

/// Wrap a call in utility dispatcher variants.
fn wrap_call_utility_variants(call: RuntimeCall) -> Vec<(RuntimeCall, &'static str)> {
	vec![
		(
			RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call.clone()] }),
			"utility::batch",
		),
		(
			RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![call.clone()] }),
			"utility::batch_all",
		),
		(
			RuntimeCall::Utility(pallet_utility::Call::force_batch { calls: vec![call.clone()] }),
			"utility::force_batch",
		),
		(
			RuntimeCall::Utility(pallet_utility::Call::as_derivative {
				index: 0,
				call: Box::new(call),
			}),
			"utility::as_derivative",
		),
	]
}

/// Assert that direct and utility-wrapper variants are rejected at validation time.
#[test]
fn wrapped_store_requires_authorization() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();

			// Fund Alice so fee checks pass and ValidateStorageCalls can reject.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: vec![42u8; 100],
			});

			// Direct: rejected for missing authorization.
			assert_eq!(
				construct_and_apply_extrinsic(Some(account.pair()), store_call.clone()),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Payment)),
				"store: direct",
			);

			// Utility wrappers: rejected because store is not allowed inside wrappers.
			for (wrapped, name) in wrap_call_utility_variants(store_call.clone()) {
				assert_eq!(
					construct_and_apply_extrinsic(Some(account.pair()), wrapped),
					Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
					"store: via {name}",
				);
			}

			// sudo_as: passes validation (sudo not inspected) but fails at dispatch
			// because no sudo key is configured in default genesis.
			let sudo_as_result = construct_and_apply_extrinsic(
				Some(account.pair()),
				RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
					who: sp_runtime::MultiAddress::Id(account.to_account_id()),
					call: Box::new(store_call),
				}),
			);
			assert!(sudo_as_result.is_ok(), "sudo_as should pass validation");
			assert!(sudo_as_result.unwrap().is_err(), "sudo_as should fail at dispatch");
		});
}

#[test]
fn wrapped_store_with_cid_config_requires_authorization() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();

			// Fund Alice so fee checks pass and ValidateStorageCalls can reject.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			let store_call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store_with_cid_config {
					cid: CidConfig { codec: 0x55, hashing: HashingAlgorithm::Blake2b256 },
					data: vec![42u8; 100],
				});

			// Direct: rejected for missing authorization.
			assert_eq!(
				construct_and_apply_extrinsic(Some(account.pair()), store_call.clone()),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Payment)),
				"store_with_cid_config: direct",
			);

			// Utility wrappers: rejected because store is not allowed inside wrappers.
			for (wrapped, name) in wrap_call_utility_variants(store_call.clone()) {
				assert_eq!(
					construct_and_apply_extrinsic(Some(account.pair()), wrapped),
					Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
					"store_with_cid_config: via {name}",
				);
			}

			// sudo_as: passes validation (sudo not inspected) but fails at dispatch
			// because no sudo key is configured in default genesis.
			let sudo_as_result = construct_and_apply_extrinsic(
				Some(account.pair()),
				RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
					who: sp_runtime::MultiAddress::Id(account.to_account_id()),
					call: Box::new(store_call),
				}),
			);
			assert!(sudo_as_result.is_ok(), "sudo_as should pass validation");
			assert!(sudo_as_result.unwrap().is_err(), "sudo_as should fail at dispatch");
		});
}

/// Store calls inside wrappers (batch, batch_all, force_batch) are rejected even when
/// authorized. Store/renew must be submitted as direct extrinsics.
#[test]
fn authorized_wrapped_store_rejected() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![42u8; 100];

			// Fund Alice for fees.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			// Authorize enough for several calls.
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				4 * data.len() as u64
			));

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});

			// Direct store should succeed.
			assert_ok_ok(construct_and_apply_extrinsic(Some(account.pair()), store_call.clone()));

			// Batch-wrapped store must be rejected.
			for (wrapped, name) in wrap_call_utility_variants(store_call) {
				assert_eq!(
					construct_and_apply_extrinsic(Some(account.pair()), wrapped),
					Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
					"{name}: wrapped store must be rejected",
				);
			}

			// Only the direct store consumed authorization (1 tx, data.len() bytes).
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: data.len() as u64,
					bytes_permanent: 0,
					bytes_allowance: 4 * data.len() as u64,
					transactions: 1,
					transactions_allowance: 0,
				},
			);
		});
}

/// Batch containing store calls is rejected — store must be submitted as direct extrinsics.
#[test]
fn batch_store_with_mixed_preimage_and_account_auth_rejected() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();

			// Fund Alice for fees.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			let data_a = vec![42u8; 100];
			let data_b = vec![99u8; 200];
			let content_hash_a = sp_io::hashing::blake2_256(&data_a);

			// Authorize preimage for data_a only.
			assert_ok!(TransactionStorage::authorize_preimage(
				RuntimeOrigin::root(),
				content_hash_a,
				data_a.len() as u64,
			));

			// Authorize account for data_b (1 transaction, enough bytes).
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data_b.len() as u64
			));

			let store_a =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data: data_a });
			let store_b =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data: data_b });

			let batch =
				RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![store_a, store_b] });

			// Batch containing store calls is rejected.
			assert_eq!(
				construct_and_apply_extrinsic(Some(account.pair()), batch),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
			);

			// Authorizations were NOT consumed (rejected before prepare).
			assert_eq!(
				TransactionStorage::preimage_authorization_extent(content_hash_a),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 100,
					transactions: 0,
					transactions_allowance: 1,
				},
				"Preimage authorization should not be consumed",
			);
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 200,
					transactions: 0,
					transactions_allowance: 0,
				},
				"Account authorization should not be consumed",
			);
		});
}

// NOTE: The following preimage/renew/authorize tests mirror those in
// bulletin-polkadot/tests/tests.rs. Keep in sync when modifying.

/// Preimage authorization allows anyone to store pre-authorized content.
#[test]
fn preimage_authorized_storage_transactions_work() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();

			// Fund Alice for transaction fees (westend has fees unlike polkadot).
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			let data = vec![0u8; 24];
			let content_hash = sp_io::hashing::blake2_256(&data);
			let call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});

			// Not authorized (no account or preimage auth) should fail to store.
			assert_eq!(
				construct_and_apply_extrinsic(Some(account.pair()), call.clone()),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Payment))
			);

			// Authorize preimage (not account).
			assert_ok!(TransactionStorage::authorize_preimage(
				RuntimeOrigin::root(),
				content_hash,
				data.len() as u64,
			));

			// Now should work via preimage authorization.
			assert_ok_ok(construct_and_apply_extrinsic(Some(account.pair()), call));

			// Verify preimage authorization was consumed.
			assert_eq!(
				TransactionStorage::preimage_authorization_extent(content_hash),
				AuthorizationExtent {
					bytes: 24,
					bytes_permanent: 0,
					bytes_allowance: 24,
					transactions: 1,
					transactions_allowance: 1,
				},
			);
		});
}

/// When both preimage and account authorizations exist, preimage takes priority.
#[test]
fn signed_store_prefers_preimage_authorization_over_account() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![0u8; 100];
			let content_hash = sp_io::hashing::blake2_256(&data);

			// Setup: authorize both account and preimage
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				500
			));
			assert_ok!(TransactionStorage::authorize_preimage(
				RuntimeOrigin::root(),
				content_hash,
				data.len() as u64,
			));

			// Store data
			let call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});
			assert_ok_ok(construct_and_apply_extrinsic(Some(account.pair()), call));

			// Verify: preimage authorization was consumed, account authorization unchanged
			assert_eq!(
				TransactionStorage::preimage_authorization_extent(content_hash),
				AuthorizationExtent {
					bytes: 100,
					bytes_permanent: 0,
					bytes_allowance: 100,
					transactions: 1,
					transactions_allowance: 1,
				},
				"Preimage authorization should be consumed"
			);
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 500,
					transactions: 0,
					transactions_allowance: 0,
				},
				"Account authorization should remain unchanged when preimage auth is used"
			);
		});
}

/// `renew` is allowed as a direct extrinsic (consuming the caller's account allowance)
/// but rejected when nested inside utility/sudo wrappers.
#[test]
fn renew_must_be_direct_extrinsic() {
	let mut t = RuntimeGenesisConfig::default().build_storage().unwrap();
	pallet_bulletin_transaction_storage::GenesisConfig::<Runtime> {
		retention_period: 100,
		byte_fee: 0,
		entry_fee: 0,
		account_authorizations: vec![],
		preimage_authorizations: vec![],
		allowed_authorizers: vec![],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	sp_io::TestExternalities::new(t).execute_with(|| {
		advance_block();
		let account = Sr25519Keyring::Alice;
		let who: AccountId = account.to_account_id();
		let data = vec![42u8; 100];

		// Fund Alice so utility::batch wrappers (not feeless) reach the wrapper rejection
		// in ValidateStorageCalls instead of failing earlier on payment. Direct store and
		// renew are themselves feeless, so the funding is irrelevant for those calls.
		use frame_support::traits::fungible::Mutate;
		Balances::mint_into(&who, 1_000_000_000_000).unwrap();
		assert_ok!(TransactionStorage::authorize_account(
			RuntimeOrigin::root(),
			who.clone(),
			0,
			data.len() as u64
		));
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(account.pair()),
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data }),
		));
		let stored_block = System::block_number();

		advance_block();

		let renew_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::force_renew {
			entry: TransactionRef::Position { block: stored_block, index: 0 },
		});

		// Direct renew succeeds with the existing account authorization.
		assert_ok_ok(construct_and_apply_extrinsic(Some(account.pair()), renew_call.clone()));
		assert_eq!(
			TransactionStorage::account_authorization_extent(who.clone()),
			AuthorizationExtent {
				bytes: 100,
				bytes_permanent: 100,
				bytes_allowance: 100,
				transactions: 2,
				transactions_allowance: 0,
			},
		);

		// Utility wrappers: rejected because renew is not allowed inside wrappers.
		for (wrapped, name) in wrap_call_utility_variants(renew_call.clone()) {
			assert_eq!(
				construct_and_apply_extrinsic(Some(account.pair()), wrapped),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
				"renew: via {name}",
			);
		}

		// sudo_as: passes validation (sudo not inspected) but fails at dispatch
		// because no sudo key is configured.
		let sudo_as_result = construct_and_apply_extrinsic(
			Some(account.pair()),
			RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
				who: sp_runtime::MultiAddress::Id(who),
				call: Box::new(renew_call),
			}),
		);
		assert!(sudo_as_result.is_ok(), "sudo_as should pass validation");
		assert!(sudo_as_result.unwrap().is_err(), "sudo_as should fail at dispatch");
	});
}

/// Non-authorizers cannot authorize_account even via batch.
#[test]
fn wrapped_authorize_account_requires_authorizer_origin() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			// Bob is not an Authorizer (only Alice is in TestAccounts).
			let attacker = Sr25519Keyring::Bob;
			let who: AccountId = attacker.to_account_id();

			// Fund for fees.
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			let call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
					who: who.clone(),
					transactions: 0,
					bytes: 1024,
				});

			// Direct: rejected at validation (BadSigner).
			assert_eq!(
				construct_and_apply_extrinsic(Some(attacker.pair()), call.clone()),
				Err(TransactionValidityError::Invalid(InvalidTransaction::BadSigner)),
			);

			// Via batch: batch itself is valid, but the inner authorize_account must
			// fail at dispatch (origin is not Authorizer). Verify via storage state.
			let batch_call =
				RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call] });
			let _ = construct_and_apply_extrinsic(Some(attacker.pair()), batch_call);
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 0,
					transactions: 0,
					transactions_allowance: 0,
				},
				"authorize_account via batch must not succeed for non-Authorizer",
			);
		});
}

/// Wrapping `authorize_account` in `batch_all` must not break the authorization.
/// The origin must remain `Signed` (not transformed to `Authorized`) so that
/// `T::Authorizer::ensure_origin()` succeeds at dispatch time.
#[test]
fn wrapped_authorize_account_succeeds() {
	let mut genesis = RuntimeGenesisConfig::default();
	genesis.transaction_storage.allowed_authorizers =
		vec![(Sr25519Keyring::Alice.to_account_id(), 1000, 100 * 1024 * 1024)];

	sp_io::TestExternalities::new(genesis.build_storage().unwrap()).execute_with(|| {
		advance_block();
		let account = Sr25519Keyring::Alice;
		let who: AccountId = account.to_account_id();
		let target: AccountId = Sr25519Keyring::Bob.to_account_id();

		// Fund Alice for batch fee overhead.
		use frame_support::traits::fungible::Mutate;
		Balances::mint_into(&who, 1_000_000_000_000).unwrap();

		// Wrap authorize_account inside batch_all — this is what the JS integration
		// test does. The origin must stay Signed(Alice) so the Authorizer check passes.
		let authorize_call =
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
				who: target.clone(),
				transactions: 10,
				bytes: 10 * 1024,
			});
		let batch_call =
			RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![authorize_call] });

		assert_ok_ok(construct_and_apply_extrinsic(Some(account.pair()), batch_call));

		// Authorization must have been created.
		assert_eq!(
			TransactionStorage::account_authorization_extent(target.clone()),
			AuthorizationExtent {
				bytes: 0,
				bytes_permanent: 0,
				bytes_allowance: 10 * 1024,
				transactions: 0,
				transactions_allowance: 10,
			},
		);

		// Now verify that the authorized target can actually store data.
		let data = vec![42u8; 100];
		let store_call =
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data: data.clone() });
		assert_ok_ok(construct_and_apply_extrinsic(Some(Sr25519Keyring::Bob.pair()), store_call));
	});
}

/// Batch containing store is rejected — store must be submitted as direct extrinsics,
/// regardless of what else is in the batch.
#[test]
fn mixed_batch_store_and_authorize_rejected() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let target: AccountId = Sr25519Keyring::Bob.to_account_id();
			let data = vec![42u8; 100];

			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			// Authorize Alice for one store.
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data.len() as u64
			));

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});
			let authorize_call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
					who: target.clone(),
					transactions: 0,
					bytes: 1024,
				});

			// Mixing store + authorize_account in a batch is rejected at validation.
			for batch_variant in [
				RuntimeCall::Utility(pallet_utility::Call::batch {
					calls: vec![store_call.clone(), authorize_call.clone()],
				}),
				RuntimeCall::Utility(pallet_utility::Call::batch_all {
					calls: vec![store_call.clone(), authorize_call.clone()],
				}),
				RuntimeCall::Utility(pallet_utility::Call::force_batch {
					calls: vec![store_call.clone(), authorize_call.clone()],
				}),
			] {
				assert_err!(
					construct_and_apply_extrinsic(Some(account.pair()), batch_variant),
					TransactionValidityError::Invalid(InvalidTransaction::Call),
				);
			}

			// Authorization was NOT consumed (rejected before prepare).
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: data.len() as u64,
					transactions: 0,
					transactions_allowance: 0,
				},
			);
		});
}

/// Batch containing store with a non-storage call is rejected — store must be direct.
#[test]
fn mixed_batch_store_and_non_storage_call_rejected() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![42u8; 100];

			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data.len() as u64
			));

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});
			let remark_call =
				RuntimeCall::System(frame_system::Call::remark { remark: vec![1, 2, 3] });

			let batch_call = RuntimeCall::Utility(pallet_utility::Call::batch {
				calls: vec![store_call, remark_call],
			});

			assert_err!(
				construct_and_apply_extrinsic(Some(account.pair()), batch_call),
				TransactionValidityError::Invalid(InvalidTransaction::Call),
			);

			// Authorization was NOT consumed.
			assert_eq!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: data.len() as u64,
					transactions: 0,
					transactions_allowance: 0,
				},
			);
		});
}

/// Deeply nested wrapper calls exceeding MAX_WRAPPER_DEPTH must be rejected.
#[test]
fn max_recursion_depth_is_enforced() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();
			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![42u8; 100];

			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&who, 1_000_000_000_000).unwrap();

			// Authorize Alice.
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data.len() as u64
			));

			// Nest store inside MAX_WRAPPER_DEPTH+1 batch wrappers.
			let mut call: RuntimeCall =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
					data: data.clone(),
				});
			for _ in 0..=pallet_bulletin_transaction_storage::MAX_WRAPPER_DEPTH {
				call = RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call] });
			}

			// Should fail with Call — store inside wrapper is rejected (the depth limit
			// in is_storage_mutating_call treats excessively nested calls as storage-mutating).
			assert_err!(
				construct_and_apply_extrinsic(Some(account.pair()), call),
				TransactionValidityError::Invalid(InvalidTransaction::Call)
			);
		});
}

/// The sudo key holder can store data via `sudo(store)` without authorization.
/// Sudo dispatches with Root origin, and `ensure_authorized` accepts Root.
#[test]
fn sudo_store_works_for_sudo_key_holder() {
	let mut t = RuntimeGenesisConfig::default().build_storage().unwrap();
	let sudo_account = Sr25519Keyring::Alice;
	pallet_sudo::GenesisConfig::<Runtime> { key: Some(sudo_account.to_account_id()) }
		.assimilate_storage(&mut t)
		.unwrap();

	sp_io::TestExternalities::new(t).execute_with(|| {
		advance_block();
		let who: AccountId = sudo_account.to_account_id();

		use frame_support::traits::fungible::Mutate;
		Balances::mint_into(&who, 1_000_000_000_000).unwrap();

		let data = vec![42u8; 100];
		let store_call =
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store { data: data.clone() });

		// sudo(store) should work — Root origin is accepted by ensure_authorized.
		let sudo_call = RuntimeCall::Sudo(pallet_sudo::Call::sudo { call: Box::new(store_call) });
		assert_ok_ok(construct_and_apply_extrinsic(Some(sudo_account.pair()), sudo_call));

		// No account authorization was needed or consumed.
		assert_eq!(
			TransactionStorage::account_authorization_extent(who),
			AuthorizationExtent {
				bytes: 0,
				bytes_permanent: 0,
				bytes_allowance: 0,
				transactions: 0,
				transactions_allowance: 0,
			},
		);
	});
}

// ============================================================================
// XCM SafeCallFilter tests — verify that storage-mutating calls are blocked
// when dispatched via XCM Transact, even with valid authorization.
// ============================================================================

/// XCM Transact with `store` must be blocked by the SafeCallFilter
/// (`EverythingBut<StorageCallInspector>`). Storage operations must go through
/// signed extrinsics, never through XCM.
#[test]
fn xcm_transact_store_is_blocked() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();

			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![42u8; 100];

			// Authorize the account so we can verify the filter blocks the call
			// regardless of authorization state.
			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data.len() as u64
			));
			assert_ne!(
				TransactionStorage::account_authorization_extent(who.clone()),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 0,
					transactions: 0,
					transactions_allowance: 0,
				},
			);

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});

			// Build an XCM message: UnpaidExecution + Transact(Superuser, store).
			// The governance location has LocationAsSuperuser in the OriginConverter,
			// so origin conversion would succeed — but SafeCallFilter must block the
			// call before that matters.
			let message: Xcm<RuntimeCall> = Xcm::builder_unsafe()
				.unpaid_execution(Unlimited, None)
				.transact(OriginKind::Superuser, None, store_call.encode())
				.build();

			let mut id = [0u8; 32];
			let outcome = xcm_executor::XcmExecutor::<
				bulletin_paseo_runtime::xcm_config::XcmConfig,
			>::prepare_and_execute(
				governance_location(), message, &mut id, Weight::MAX, Weight::MAX
			);

			// SafeCallFilter returns false for store → XcmError::NoPermission
			assert!(
				outcome.clone().ensure_complete().is_err(),
				"XCM Transact store must be blocked by SafeCallFilter, got: {outcome:?}",
			);

			// Authorization must not have been consumed.
			assert_ne!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 0,
					transactions: 0,
					transactions_allowance: 0,
				},
				"Authorization should remain unconsumed since XCM was blocked",
			);
		});
}

/// XCM Transact with `store` wrapped in `utility::batch` must also be blocked.
/// The `StorageCallInspector` recursively inspects inner calls.
#[test]
fn xcm_transact_wrapped_store_is_blocked() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();

			let account = Sr25519Keyring::Alice;
			let who: AccountId = account.to_account_id();
			let data = vec![42u8; 100];

			assert_ok!(TransactionStorage::authorize_account(
				RuntimeOrigin::root(),
				who.clone(),
				0,
				data.len() as u64
			));

			let store_call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::store {
				data: data.clone(),
			});
			let batch_call =
				RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![store_call] });

			let message: Xcm<RuntimeCall> = Xcm::builder_unsafe()
				.unpaid_execution(Unlimited, None)
				.transact(OriginKind::Superuser, None, batch_call.encode())
				.build();

			let mut id = [0u8; 32];
			let outcome = xcm_executor::XcmExecutor::<
				bulletin_paseo_runtime::xcm_config::XcmConfig,
			>::prepare_and_execute(
				governance_location(), message, &mut id, Weight::MAX, Weight::MAX
			);

			assert!(
				outcome.clone().ensure_complete().is_err(),
				"XCM Transact batch(store) must be blocked by recursive SafeCallFilter, got: {outcome:?}",
			);

			// Authorization must not have been consumed.
			assert_ne!(
				TransactionStorage::account_authorization_extent(who),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 0,
					transactions: 0,
					transactions_allowance: 0,
				},
			);
		});
}

/// XCM Transact with `authorize_account` must succeed — management calls are
/// allowed through XCM (they are not storage-mutating).
#[test]
fn xcm_transact_authorize_account_works() {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			advance_block();

			let target: AccountId = Sr25519Keyring::Ferdie.to_account_id();

			// Verify no authorization exists yet.
			assert_eq!(
				TransactionStorage::account_authorization_extent(target.clone()),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 0,
					transactions: 0,
					transactions_allowance: 0,
				},
			);

			let authorize_call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
					who: target.clone(),
					transactions: 0,
					bytes: 1024,
				});

			let message: Xcm<RuntimeCall> = Xcm::builder_unsafe()
				.unpaid_execution(Unlimited, None)
				.transact(OriginKind::Superuser, None, authorize_call.encode())
				.build();

			let mut id = [0u8; 32];
			let outcome = xcm_executor::XcmExecutor::<
				bulletin_paseo_runtime::xcm_config::XcmConfig,
			>::prepare_and_execute(
				governance_location(), message, &mut id, Weight::MAX, Weight::MAX
			);

			assert!(
				outcome.clone().ensure_complete().is_ok(),
				"XCM Transact authorize_account must succeed, got: {outcome:?}",
			);

			// Authorization must have been created.
			assert_eq!(
				TransactionStorage::account_authorization_extent(target),
				AuthorizationExtent {
					bytes: 0,
					bytes_permanent: 0,
					bytes_allowance: 1024,
					transactions: 0,
					transactions_allowance: 0,
				},
			);
		});
}

/// XCM Transact from an AssetHub smart contract: an `AccountKey20`-descended
/// sibling origin (e.g. a pallet-revive contract) converts to a `Signed` origin
/// via `HashedDescription`. Registration in `AllowedAuthorizers` gates dispatch;
/// on success the hashed-account budget is decremented.
#[test]
fn xcm_transact_authorize_account_from_asset_hub_contract() {
	use bulletin_paseo_runtime::paseo_constants::system_parachain::ASSET_HUB_ID;

	let contract_addr = [0xAAu8; 20];
	let hashed: AccountId =
		LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
			Location::new(
				1,
				[Parachain(ASSET_HUB_ID), AccountKey20 { network: None, key: contract_addr }],
			)
			.into(),
		)
		.expect("HashedDescription resolves sibling+AccountKey20");

	let target = Sr25519Keyring::Ferdie.to_account_id();
	let (txs_budget, bytes_budget) = (1000u32, 100 * 1024 * 1024u64);
	let (txs, bytes) = (3u32, 1024u64);

	let zero = AuthorizationExtent {
		bytes: 0,
		bytes_permanent: 0,
		bytes_allowance: 0,
		transactions: 0,
		transactions_allowance: 0,
	};

	let execute_xcm = |registered: bool| {
		let mut genesis = RuntimeGenesisConfig::default();
		if registered {
			genesis.transaction_storage.allowed_authorizers =
				vec![(hashed.clone(), txs_budget, bytes_budget)];
		}
		sp_io::TestExternalities::new(genesis.build_storage().unwrap()).execute_with(|| {
			let call =
				RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
					who: target.clone(),
					transactions: txs,
					bytes,
				});
			let message: Xcm<RuntimeCall> = Xcm::builder_unsafe()
				.unpaid_execution(Unlimited, None)
				.descend_origin(Junctions::from([AccountKey20 {
					network: None,
					key: contract_addr,
				}]))
				.transact(OriginKind::SovereignAccount, None, call.encode())
				.build();
			let mut id = [0u8; 32];
			xcm_executor::XcmExecutor::<bulletin_paseo_runtime::xcm_config::XcmConfig>::prepare_and_execute(
				Location::new(1, [Parachain(ASSET_HUB_ID)]),
				message,
				&mut id,
				Weight::MAX,
				Weight::MAX,
			);
			(
				TransactionStorage::account_authorization_extent(target.clone()),
				pallet_bulletin_transaction_storage::AllowedAuthorizers::<Runtime>::get(&hashed),
			)
		})
	};

	// Unregistered: inner dispatch fails with BadOrigin; no authorization created.
	assert_eq!(execute_xcm(false), (zero, None));

	// Registered: dispatch succeeds; target gains allowance and contract budget shrinks.
	let (extent, budget) = execute_xcm(true);
	assert_eq!(
		extent,
		AuthorizationExtent { bytes_allowance: bytes, transactions_allowance: txs, ..zero },
	);
	let quota = budget
		.expect("hashed contract account still registered after partial spend")
		.quota
		.expect("authorizer has a tracked quota");
	assert_eq!(quota.transactions, txs_budget - txs);
	assert_eq!(quota.bytes, bytes_budget - bytes);
}

fn default_authorizer_budget() -> pallet_bulletin_transaction_storage::AuthorizerBudget<BlockNumber>
{
	pallet_bulletin_transaction_storage::AuthorizerBudget {
		quota: Some(pallet_bulletin_transaction_storage::Quota {
			transactions: 1000,
			bytes: 100 * 1024 * 1024,
		}),
		valid_until: None,
		feeless: false,
	}
}

#[test]
fn sudo_round_trips_authorizer_membership() {
	// Lifecycle: sudo adds Eve → Eve authorizes → sudo removes Eve → Eve rejected.
	// Eve isn't in `TestAccounts` (Alice + EXTRA_AUTHORIZER), so removal genuinely
	// strips her authorize rights.
	let mut genesis = RuntimeGenesisConfig::default();
	genesis.transaction_storage.allowed_authorizers = vec![];
	genesis.sudo.key = Some(Sr25519Keyring::Alice.to_account_id());

	sp_io::TestExternalities::new(genesis.build_storage().unwrap()).execute_with(|| {
		let alice = Sr25519Keyring::Alice;
		let eve = Sr25519Keyring::Eve;
		let target = Sr25519Keyring::Ferdie;
		use frame_support::traits::fungible::Mutate;
		Balances::mint_into(&alice.to_account_id(), 1_000_000_000_000).unwrap();
		Balances::mint_into(&eve.to_account_id(), 1_000_000_000_000).unwrap();

		let sudo = |inner: RuntimeCall| -> RuntimeCall {
			RuntimeCall::Sudo(pallet_sudo::Call::<Runtime>::sudo { call: Box::new(inner) })
		};
		let authorize =
			RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::authorize_account {
				who: target.to_account_id(),
				transactions: 5,
				bytes: 1024,
			});

		// Add Eve via sudo, then Eve authorizes.
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(alice.pair()),
			sudo(RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::add_authorizer {
				who: eve.to_account_id(),
				budget: default_authorizer_budget(),
			})),
		));
		assert!(pallet_bulletin_transaction_storage::AllowedAuthorizers::<Runtime>::contains_key(
			eve.to_account_id(),
		));
		assert_ok_ok(construct_and_apply_extrinsic(Some(eve.pair()), authorize.clone()));
		assert_eq!(
			TransactionStorage::account_authorization_extent(target.to_account_id()),
			AuthorizationExtent {
				bytes: 0,
				bytes_permanent: 0,
				bytes_allowance: 1024,
				transactions: 0,
				transactions_allowance: 5,
			},
		);

		// Sudo removes Eve → her subsequent authorize is rejected as BadSigner.
		assert_ok_ok(construct_and_apply_extrinsic(
			Some(alice.pair()),
			sudo(RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::remove_authorizer {
				who: eve.to_account_id(),
			})),
		));
		assert_eq!(
			construct_and_apply_extrinsic(Some(eve.pair()), authorize),
			Err(transaction_validity::TransactionValidityError::Invalid(
				InvalidTransaction::BadSigner,
			)),
		);
	});
}

#[test]
fn non_sudo_cannot_add_authorizer() {
	// Bob (not sudo) calls `add_authorizer` directly — `AuthorizerRegistrarOrigin`
	// is enforced at dispatch, so the outer apply Ok but dispatch returns BadOrigin.
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
		.execute_with(|| {
			let bob = Sr25519Keyring::Bob;
			use frame_support::traits::fungible::Mutate;
			Balances::mint_into(&bob.to_account_id(), 1_000_000_000_000).unwrap();

			let call = RuntimeCall::TransactionStorage(TxStorageCall::<Runtime>::add_authorizer {
				who: Sr25519Keyring::Eve.to_account_id(),
				budget: default_authorizer_budget(),
			});
			let applied = construct_and_apply_extrinsic(Some(bob.pair()), call)
				.expect("validation passes (balance present)");
			assert_eq!(applied.map(|_| ()), Err(DispatchError::BadOrigin));
		});
}
