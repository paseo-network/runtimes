# If CHAIN_SPEC_PACKAGES is set via environment variable, use it
# Otherwise, default to all packages
if [ -n "${CHAIN_SPEC_PACKAGES:-}" ]; then
  # Convert space-separated string to array
  read -ra PACKAGES <<< "$CHAIN_SPEC_PACKAGES"
  echo "📦 Generating chain specs for selected packages: ${PACKAGES[*]}"
else
  PACKAGES=(
    "paseo-local"
    "paseo-dev"
    "asset-hub-paseo-local"
    "bridge-hub-paseo-local"
    "collectives-paseo-local"
    "people-paseo-local"
    "coretime-paseo-local"
    "bulletin-paseo-local"
  )
  echo "📦 Generating chain specs for all packages"
fi

get_package_params() {
  local pkg="$1"

  ## Clean variables just in case
  NAME="" ID="" PARA_ID="" RUNTIME="" RELAY="" PROTOCOL_ID="" TYPE="" CHAIN="" SS58=""
  case "$pkg" in
    paseo-local)
      NAME="Paseo Local Testnet"
      ID="paseo-local"
      RUNTIME="relay/paseo"
      PROTOCOL_ID="pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    paseo-dev)
      NAME="Paseo Dev"
      ID="paseo-dev"
      RUNTIME="relay/paseo"
      PROTOCOL_ID="pas"
      TYPE="development"
      CHAIN="development"
    ;;
    # Substitute relay — fresh from-genesis Paseo relay (4 community-provider bootstrap validators,
    # ah_client Passive). Presents AS Paseo (name/id "paseo") since it replaces it; only the
    # protocol-id differs ("pad" = polkadot apps devenet). Not in the default PACKAGES list; build:
    #   CHAIN_SPEC_PACKAGES="paseo-substitute" ./scripts/generate_chain_specs.sh
    paseo-substitute)
      NAME="Paseo"
      ID="paseo"
      RUNTIME="relay/paseo"
      PROTOCOL_ID="pad"
      TYPE="live"
      CHAIN="substitute"
      SS58=42
    ;;
    asset-hub-paseo-local)
      NAME="Asset Hub Paseo Local"
      ID="asset-hub-paseo-local"
      PARA_ID=1000
      RUNTIME="system-parachains/asset-hub-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="ah-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    bridge-hub-paseo-local)
      NAME="Bridge Hub Paseo Local"
      ID="paseo-bridge-hub-local"
      PARA_ID=1002
      RUNTIME="system-parachains/bridge-hub-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="bh-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    collectives-paseo-local)
      NAME="Collectives Paseo Local"
      ID="paseo-collectives-local"
      PARA_ID=1001
      RUNTIME="system-parachains/collectives-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="col-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    people-paseo-local)
      NAME="People Paseo Local"
      ID="paseo-people-local"
      PARA_ID=1004
      RUNTIME="system-parachains/people-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="pc-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    coretime-paseo-local)
      NAME="Coretime Paseo Local"
      ID="paseo-coretime-local"
      PARA_ID=1005
      RUNTIME="system-parachains/coretime-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="ct-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    bulletin-paseo-local)
      NAME="Bulletin Paseo Local"
      ID="bulletin-paseo-local"
      PARA_ID=1501
      RUNTIME="system-parachains/bulletin-paseo"
      RELAY="paseo-local"
      PROTOCOL_ID="bl-pas"
      TYPE="local"
      CHAIN="local_testnet"
    ;;
    *)
      echo "⚠️  No config found for $pkg"
      return 1
    ;;
  esac
}

for pkg in "${PACKAGES[@]}"; do
  echo "🚀 Generating spec for $pkg..."
  get_package_params "$pkg"

  ARGS=(
    --profile release
    --skip-build
    --raw
    --name "$NAME"
    --id "$ID"
    --type "$TYPE"
    --chain "$CHAIN"
    --output "chain-specs/${pkg}.json"
    --properties ss58Format=${SS58:-0},tokenDecimals=10,tokenSymbol="PAS"
    --protocol-id "$PROTOCOL_ID"
    --default-bootnode=false
    --genesis-code=false
    --genesis-state=false
    --deterministic=false
    --runtime "$RUNTIME"
  )

  [[ -n "${PARA_ID:-}" ]] && ARGS+=(--para-id "$PARA_ID")
  [[ -n "${RELAY:-}" ]] && ARGS+=(--relay "$RELAY")
  [[ -z "${PARA_ID:-}" && -z "${RELAY:-}" ]] && ARGS+=(--is-relay)

  ## Generate specs with Pop-CLI: https://github.com/r0gue-io/pop-cli
  pop build spec "${ARGS[@]}"

  echo "✅ Spec generated for: ${pkg}"
done

## Only interested in the raw files
find chain-specs -type f -name "*.json" ! -name "*-raw.json" -exec rm -f {} \;

for f in chain-specs/*-raw.json; do
  [ -e "$f" ] || continue
  mv "$f" "${f%-raw.json}.json"
done

echo "✅ Chain specs correctly saved"
