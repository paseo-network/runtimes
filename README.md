# Paseo Runtimes

Runtime definitions for the Paseo testnet. This repository contains the relay
runtime, system parachain runtimes, chain specs, and helper scripts used to keep
Paseo aligned with Polkadot SDK changes.

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

## Testnet vs Production

Paseo tracks the production networks closely, but it is still a testnet. Treat
the tables below as a quick check before carrying assumptions from Paseo to
Kusama or Polkadot.

### Features

| Feature | Paseo | Kusama | Polkadot |
| ---  | ---- | ---- | ---- |
| [Agile Coretime](https://polkadot-fellows.github.io/RFCs/approved/0001-agile-coretime.html?highlight=agile#rfc-1-agile-coretime) | ✅ | ✅ | ✅ |
| [Async Backing](https://wiki.polkadot.com/learn/learn-async-backing/#asynchronous-backing) | ✅ | ✅ | ✅ |
| [Elastic Scaling](https://polkadot-fellows.github.io/RFCs/approved/0103-introduce-core-index-commitment.html?highlight=Elastic%20scaling#summary) | ✅ | ✅ | ✅ |
| Asset Hub Migration | ✅ | ✅ | ✅ |
| PolkaVM Contracts | ✅ | ✅ | ✅  |
| Contracts: [ERC-20 Precompile](https://github.com/paritytech/polkadot-sdk/tree/master/substrate/frame/assets/precompiles) | ✅ | ✅ | ✅ |
| Contracts: [XCM Precompile](https://github.com/paritytech/polkadot-sdk/tree/master/polkadot/xcm/pallet-xcm/precompiles/) | ✅ | ✅ | ✅ |
| Coretime interlude period | 2 days | 7 days | 7 days |


### Costs

| Feature | Paseo (PAS) | Kusama (KSM) | Polkadot (DOT) |
| ---  | ---- | ---- | ---- |
| Parachain Id Reservation | 100 | 4 | 100 |
| Parachain Registration | ~3,200 | ~105 | ~3,200 |
| Asset Creation | ~0.0017 + 0.4 Deposit | ~0.00012 + 0.013 Deposit | ~0.0018 + 0.4 Deposit |
| Identity Creation | ~0.002 + 0.2 Deposit | ~0.00009 + 0.006 Deposit | ~0.002 + 0.2 Deposit |
| Contract Instantiation (12K polkavm blob) | ~0.12 + 0.6 Deposit | ~0.004 + 0.02 Deposit | - |

## Monitoring

The relay block production monitoring design is documented in
`paseo-relay-block-production-slo-design.md`. Keep monitoring docs tied to the
exporter, Prometheus rules, and runtime constants they describe.
