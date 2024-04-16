---
name: 'Runtime Upgrade: [Upgrade Title/Version]'
about: Checklist for tracking the progress of a runtime upgrade on the Paseo Testnet.
title: ''
labels: runtime-upgrade
assignees: ''

---

# Paseo Testnet Runtime Upgrade Checklist

## Preparation Tasks

- [ ] **Upgrade Proposal Review**: Confirm that the core team agreed on the upcoming Runtime Upgrade.
- [ ] **Impact Analysis Completed**: Which would be the impact of this Runtime Upgrade?.
- [ ] **PR Creation:**: Run the script to backport the changes from the Polkadot Fellowship Runtime and solve all conflicts. 
- [ ] **Benchmarking**: Run benchmarks according to Paseo hardware specs and upload them as a different PR targeting the previous PR mentioned.
- [ ] **Chopsticks testing:** Fork Paseo relay-chain and its system chains and commit the targeted runtime upgrade.
  - [ ] **Epoch and Era changes:** Make chopticks to produce the necessary amount of changes to change the Era and assure blocks are still produced.
  - [ ] **XCM transfers:** Make a teleport to the Paseo assethub and PAS reserved transfer to any available parachain.
- [ ] **Try-Runtime**: Run try-runtime and mke sure that no state inconsistences are reported.
- [ ] **Polkadot Fellowship contact:** In case of any issues detected during testing, contact any Polkadot Fellowship member to address the concern. If no issues this can just be checked.
- [ ] **Runtime upgrade on test instance:** Commit the Runtime upgrade on the Paseo test instance and leave it running for a whole day.

## Execution

- [ ] **Notify Stakeholders**: Distribute the communication plan through all relevant channels.
- [ ] **Schedule Upgrade Window**: Decide on and communicate the upgrade window to all stakeholders. Use always CET Timezone for scheduling or communicating deadlines.
- [ ] **Final Pre-Upgrade Check**: Conduct a last-minute check to ensure all systems and stakeholders are ready.
- [ ] **Commit Upgrade**: Initiate the runtime upgrade process according to the planned procedure. Use always CET Timezone for scheduling or communicating deadlines.
- [ ] **Monitor Deployment**: Closely monitor the network for any immediate issues during the upgrade.

## Validation

- [ ] **Verify Network Stability**: Check that the network returns to normal operation post-upgrade.
- [ ] **Confirm Upgrade Success**: Validate that the new runtime version is correctly implemented across the network.
- [ ] **Stakeholder Feedback**: Collect and review feedback from validators, developers, and users.

## Post-Upgrade

- [ ] **Update Documentation**: Revise all relevant documentation to reflect changes introduced by the upgrade.
- [ ] **Announce Completion**: Inform stakeholders that the upgrade has been successfully completed.
- [ ] **Post-Upgrade Review**: Conduct a review meeting to discuss the upgrade process, identify any issues, and document lessons learned.

## Troubleshooting

- [ ] **Issue Tracking**: Log any issues encountered during the upgrade in a dedicated tracker.
- [ ] **Resolve Critical Issues**: Prioritize and address any critical issues that arise.
- [ ] **Communicate Ongoing Issues**: Keep stakeholders informed about any unresolved issues and the plans for resolution.
