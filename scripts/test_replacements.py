import json
import re
import sys
from revert_unwanted_changes import load_replacements, apply_filters

def test_replacements(config_file):
    # Load replacements
    regex_replacements, literal_replacements, _ = load_replacements(config_file)

    # Test cases and expected results combined
    test_cases = [
        (
         "Polkadot is a blockchain network.",
         "Paseo is a blockchain network."
        ),
        (
         "The Polkadot Fellowship is responsible for governance.",
         "The Paseo Core Team is responsible for governance."
        ),
        (
         "Check out https://github.com/polkadot-fellows/runtimes.git for more info.",
         "Check out https://github.com/paseo-network/runtimes.git for more info."
        ),
        (
         "We use polkadot_runtime_constants in our code.",
         "We use paseo_runtime_constants in our code."
        ),
        (
         "The module is named polkadot-runtime-constants.",
         "The module is named paseo-runtime-constants."
        ),
        (
         "Import runtime::polkadot for full functionality.",
         "Import runtime::paseo for full functionality."
        ),
        (
         "Dependencies include \"polkadot-runtime\" version 0.9.0",
         "Dependencies include \"paseo-runtime\" version 0.9.0"
        ),
        (
         "spec_name: create_runtime_str!(\"polkadot\"),",
         "spec_name: create_runtime_str!(\"paseo\"),"
        ),
        (
         "impl_name: create_runtime_str!(\"parity-polkadot\"),",
         "impl_name: create_runtime_str!(\"paseo-testnet\"),"
        ),
        (
         "type LeaseOffset = LeaseOffset;",
         "type LeaseOffset = ();"
        ),
        (
         "// Polkadot version identifier;",
         "// Portico version identifier;"
        ),
        (
         "/// Runtime version (Polkadot).",
         "/// Runtime version (Paseo)."
        ),
        (
         "//! XCM configuration for Polkadot.",
         "//! XCM configuration for Paseo."
        ),
        (
         "pub const EPOCH_DURATION_IN_SLOTS: BlockNumber = prod_or_fast!(4 * HOURS, 1 * MINUTES);",
         "pub const EPOCH_DURATION_IN_SLOTS: BlockNumber = prod_or_fast!(1 * HOURS, 1 * MINUTES);"
        ),
        (
         "impl<T: frame_system::Config> a::b::c::WeightInfo<T> for WeightInfo<T> { ",
         "impl<T: frame_system::Config> WeightInfo<T> {"
        ),
        (
         "impl<T: frame_system::Config> single::WeightInfo<T> for WeightInfo<T> { ",
         "impl<T: frame_system::Config> WeightInfo<T> {"
        ),
    ]

    # Run tests
    total_tests = len(test_cases)
    passed_tests = 0

    for i, (test, expected) in enumerate(test_cases):
        result = apply_filters(test, regex_replacements, literal_replacements)
        if result == expected:
            print(f"Test case {i+1}: PASSED")
            passed_tests += 1
        else:
            print(f"Test case {i+1}: FAILED")
            print(f"  Input:    {test}")
            print(f"  Expected: {expected}")
            print(f"  Got:      {result}")
        print()

    # Print summary
    print("=" * 40)
    print(f"Test Summary: {passed_tests}/{total_tests} tests passed")
    if passed_tests == total_tests:
        print("All tests passed successfully!")
    else:
        print(f"WARNING: {total_tests - passed_tests} test(s) failed.")

if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python test_replacements.py <path_to_replacements_config.json>")
        sys.exit(1)

    config_file = sys.argv[1]
    test_replacements(config_file)