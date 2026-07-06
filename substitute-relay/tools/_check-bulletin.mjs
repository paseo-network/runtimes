import { ApiPromise, WsProvider } from '@polkadot/api';

const BULLETIN = 'wss://paseo-bulletin-rpc.polkadot.io';

async function main() {
  const api = await ApiPromise.create({ provider: new WsProvider(BULLETIN) });

  console.log('chain:', (await api.rpc.system.chain()).toString());
  console.log('runtime:', api.runtimeVersion.specName.toString(), api.runtimeVersion.specVersion.toString());
  const header = await api.rpc.chain.getHeader();
  console.log('best header number:', header.number.toString());

  console.log('parachainInfo.parachainId:', (await api.query.parachainInfo.parachainId()).toString());

  console.log('collatorSelection invulnerables:', (await api.query.collatorSelection.invulnerables()).toString());

  try {
    const validationData = await api.query.parachainSystem.validationData();
    console.log('validationData (relay parent info):', validationData.toString());
  } catch(e) { console.log('validationData err', e.message); }

  try {
    const lastRelayChainBlockNumber = await api.query.parachainSystem.lastRelayChainBlockNumber?.();
    console.log('lastRelayChainBlockNumber:', lastRelayChainBlockNumber?.toString());
  } catch(e) {}

  console.log('MaxPermanentStorageSize / PermanentStorageUsed:');
  try {
    console.log(' used:', (await api.query.transactionStorage.permanentStorageUsed()).toString());
  } catch(e) { console.log(' err', e.message); }
  try {
    console.log(' retentionPeriod:', (await api.query.transactionStorage.retentionPeriod()).toString());
  } catch(e) { console.log(' err', e.message); }

  console.log('sudo key:', (await api.query.sudo.key()).toString());

  await api.disconnect();
  process.exit(0);
}

main().catch((e) => { console.error('FATAL', e.message); process.exit(1); });
