# Runtimes

Runtimes for Polkadot's community testnet.

## Structure

```pre
├── relay
│   ├── paseo
└── system-parachains
    ├── asset-hub-paseo
    │    ├── bp-asset-hub-paseo
    ├── bridge-hub-paseo
    │    ├── bp-bridge-hub-paseo
    ├── people-paseo
    ├── coretime-paseo
```
---

## Testnet vs Production

The following section presents the differences between testnet and production runtimes. By showing users the variations in available features and operational costs, users can better adjust their expectations when transitioning from testnet to production environments.

### Features

| Feature | Paseo | Kusama | Polkadot |
| ---  | ---- | ---- | ---- |
| [Agile Coretime](https://polkadot-fellows.github.io/RFCs/approved/0001-agile-coretime.html?highlight=agile#rfc-1-agile-coretime) | ✅ | ✅ | ✅ |
| [Async Backing](https://wiki.polkadot.com/learn/learn-async-backing/#asynchronous-backing) | ✅ | ✅ | ✅ |
| [Elastic Scaling](https://polkadot-fellows.github.io/RFCs/approved/0103-introduce-core-index-commitment.html?highlight=Elastic%20scaling#summary) | ✅ | ✅ | ✅ |
| Asset Hub Migration | ✅ | ✅ | ✅ |
| PolkaVM Contracts | ✅ | ✅ | ❌ |
| Contracts: [ERC-20 Precompile](https://github.com/paritytech/polkadot-sdk/tree/master/substrate/frame/assets/precompiles) | ✅ | ✅ | ❌ |
| Contracts: [XCM Precompile](https://github.com/paritytech/polkadot-sdk/tree/master/polkadot/xcm/pallet-xcm/precompiles/) | ✅ | ✅ | ❌ |
| Coretime interlude period | 2 days | 7 days | 7 days |


### Costs

| Feature | Paseo (PAS) | Kusama (KSM) | Polkadot (DOT) |
| ---  | ---- | ---- | ---- |
| Parachain Id Reservation | 100 | 4 | 100 |
| Parachain Registration | ~3,200 | ~105 | ~3,200 |
| Asset Creation | ~0.0017 + 0.4 Deposit | ~0.00012 + 0.013 Deposit | ~0.0018 + 0.4 Deposit |
| Identity Creation | ~0.002 + 0.2 Deposit | ~0.00009 + 0.006 Deposit | ~0.002 + 0.2 Deposit |
| Contract Instantiation (12K polkavm blob) | ~0.12 + 0.6 Deposit | ~0.004 + 0.02 Deposit | ❌ |
