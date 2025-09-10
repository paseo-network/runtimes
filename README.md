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
| [RFC-103](https://github.com/polkadot-fellows/RFCs/pull/103) | ✅ | ✅ | ❌ |
| Executed Asset Hub Migration | ✅ | ❌ | ❌ |
| Deployed Pallet Revive | ✅ | ✅ | ❌ |
| ERC-20 Precompile | ✅ | ❌ | ❌ |
| XCM Precompile | ✅ | ❌ | ❌ |


### Costs

| Feature | Paseo (PAS) | Kusama (KSM) | Polkadot (DOT) |
| ---  | ---- | ---- | ---- |
| Parachain Id Reservation | 100 | 4 | 100 |
| Parachain Registration | ~3,200 | ~105 | ~3,200 |
| Asset Creation | ~0.0017 + 0.4 Deposit | ~0.00012 + 0.013 Deposit | ~0.0018 + 0.4 Deposit |
| Identity Creation | ~0.002 + 0.2 Deposit | ~0.00009 + 0.006 Deposit | ~0.002 + 0.2 Deposit |
| Contract Instantiation (12K polkavm blob) | ~0.12 + 0.6 Deposit | ~0.004 + 0.02 Deposit | ❌ |
