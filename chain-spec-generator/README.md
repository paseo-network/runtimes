# Paseo chain spec generator

This tool is the actual means to generate Paseo Testnet relay chain spec together with its system chains specs.

### Build instructions

`cargo build -r -p chain-spec-generator`

### Run instructions

To generate paseo relay chain spec:

`chain-spec-generator paseo --raw > paseo-raw.json`