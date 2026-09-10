#!/usr/bin/env bash
#
# Dependencies for `/bench` runs on the self-hosted benchmark runner.
#
#   ./.github/scripts/bench-deps.sh check     # (default) verify the host is provisioned - run by CI
#   ./.github/scripts/bench-deps.sh install   # one-off host provisioning - run by hand, needs sudo
#
# The benchmark runner is a long-lived machine, so CI does not apt-install on every run: it only
# checks and fails early with an actionable message. `install` is the counterpart you run once
# (and again whenever this list grows).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../env"

APT_PACKAGES=(
  build-essential
  clang
  cmake
  curl
  git
  jq
  libclang-dev
  libssl-dev
  lz4
  make
  pkg-config
  protobuf-compiler
  python3
)

# binary -> apt package providing it
declare -A REQUIRED_BINS=(
  [cc]=build-essential
  [clang]=clang
  [cmake]=cmake
  [curl]=curl
  [git]=git
  [jq]=jq
  [make]=make
  [pkg-config]=pkg-config
  [protoc]=protobuf-compiler
  [python3]=python3
)

install_omni_bencher() {
  local dest="$1"
  echo "-- installing frame-omni-bencher $FRAME_OMNI_BENCHER_RELEASE_VERSION into $dest"
  mkdir -p "$dest"
  curl -Lsf --show-error --retry 5 --retry-delay 10 --retry-all-errors \
    --connect-timeout 10 --max-time 600 \
    --output "$dest/frame-omni-bencher" \
    "https://github.com/paritytech/polkadot-sdk/releases/download/${FRAME_OMNI_BENCHER_RELEASE_VERSION}/frame-omni-bencher"
  chmod +x "$dest/frame-omni-bencher"
  "$dest/frame-omni-bencher" --version
}

cmd="${1:-check}"

case "$cmd" in
install)
  sudo apt-get update
  sudo apt-get install --assume-yes "${APT_PACKAGES[@]}"

  if ! command -v cargo >/dev/null 2>&1; then
    echo "!! rustup/cargo is not installed for user $(whoami)."
    echo "!! Install it with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    exit 1
  fi

  # subweight is used to diff the regenerated weights against main.
  cargo install subweight --locked

  install_omni_bencher "$HOME/.local/bin"
  echo
  echo "Make sure $HOME/.local/bin is on PATH for the runner service user."
  ;;

check)
  missing_pkgs=()
  for bin in "${!REQUIRED_BINS[@]}"; do
    if ! command -v "$bin" >/dev/null 2>&1; then
      missing_pkgs+=("${REQUIRED_BINS[$bin]}")
    fi
  done

  if [[ ${#missing_pkgs[@]} -gt 0 ]]; then
    # shellcheck disable=SC2207
    missing_pkgs=($(printf '%s\n' "${missing_pkgs[@]}" | sort -u))
    echo "❌ The benchmark runner is missing: ${missing_pkgs[*]}"
    echo "   Provision it once with: ./.github/scripts/bench-deps.sh install"
    exit 1
  fi

  # frame-omni-bencher is pinned in .github/env, so re-download it whenever the pin moves
  # instead of trusting whatever version happens to be on the host.
  want="$FRAME_OMNI_BENCHER_RELEASE_VERSION"
  stamp="$HOME/.local/share/frame-omni-bencher.version"
  if [[ ! -x "$HOME/.local/bin/frame-omni-bencher" ]] || [[ "$(cat "$stamp" 2>/dev/null || true)" != "$want" ]]; then
    install_omni_bencher "$HOME/.local/bin"
    mkdir -p "$(dirname "$stamp")"
    echo "$want" >"$stamp"
  else
    echo "-- frame-omni-bencher $want already installed"
  fi
  echo "$HOME/.local/bin" >>"${GITHUB_PATH:-/dev/null}"

  if ! command -v subweight >/dev/null 2>&1; then
    if ! command -v cargo >/dev/null 2>&1; then
      echo "❌ cargo is not on PATH, cannot install subweight"
      exit 1
    fi
    echo "-- installing subweight"
    cargo install subweight --locked
  fi

  echo "✅ benchmark runner dependencies are in place"
  ;;

*)
  echo "usage: $0 [check|install]" >&2
  exit 1
  ;;
esac
