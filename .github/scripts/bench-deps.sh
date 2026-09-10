#!/usr/bin/env bash
#
# Dependencies for `/bench` runs on the self-hosted benchmark runner.
#
#   ./.github/scripts/bench-deps.sh ensure    # (default) install whatever is missing - run by CI
#   ./.github/scripts/bench-deps.sh check     # verify only, change nothing
#   ./.github/scripts/bench-deps.sh install   # force a full (re)install
#
# `ensure` is what CI runs. It installs only what is actually missing, so a provisioned runner
# does no apt work at all and the step costs a second or two. The runner gets re-registered from
# outside this repo and can come back as a bare machine, so the workflow has to be able to heal
# itself rather than relying on the host being prepared by hand.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../env"

# Checked with dpkg rather than `command -v`, so that packages shipping only headers
# (libssl-dev, libclang-dev) are verified too - those have no binary to look for.
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

BIN_DIR="$HOME/.local/bin"
OMNI_BENCHER_STAMP="$HOME/.local/share/frame-omni-bencher.version"

missing_packages() {
  local pkg missing=()
  for pkg in "${APT_PACKAGES[@]}"; do
    if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
      missing+=("$pkg")
    fi
  done
  printf '%s\n' "${missing[@]:-}"
}

apt_install() {
  local pkgs=("$@")
  echo "-- installing: ${pkgs[*]}"

  # `sudo -n` so a runner without passwordless sudo fails immediately instead of blocking on a
  # password prompt until the job times out.
  if ! sudo -n true 2>/dev/null; then
    echo "::error::Missing packages (${pkgs[*]}) but this runner has no passwordless sudo."
    echo "::error::Install them on the host by hand: sudo apt-get install -y ${pkgs[*]}"
    exit 1
  fi

  sudo -n env DEBIAN_FRONTEND=noninteractive apt-get update
  sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install --assume-yes --no-install-recommends "${pkgs[@]}"
}

install_omni_bencher() {
  echo "-- installing frame-omni-bencher $FRAME_OMNI_BENCHER_RELEASE_VERSION into $BIN_DIR"
  mkdir -p "$BIN_DIR"
  curl -Lsf --show-error --retry 5 --retry-delay 10 --retry-all-errors \
    --connect-timeout 10 --max-time 600 \
    --output "$BIN_DIR/frame-omni-bencher" \
    "https://github.com/paritytech/polkadot-sdk/releases/download/${FRAME_OMNI_BENCHER_RELEASE_VERSION}/frame-omni-bencher"
  chmod +x "$BIN_DIR/frame-omni-bencher"
  "$BIN_DIR/frame-omni-bencher" --version
  mkdir -p "$(dirname "$OMNI_BENCHER_STAMP")"
  echo "$FRAME_OMNI_BENCHER_RELEASE_VERSION" >"$OMNI_BENCHER_STAMP"
}

# frame-omni-bencher is pinned in .github/env, so it is re-downloaded whenever the pin moves
# instead of trusting whatever version happens to be on the host.
ensure_omni_bencher() {
  local have
  have="$(cat "$OMNI_BENCHER_STAMP" 2>/dev/null || true)"
  if [[ -x "$BIN_DIR/frame-omni-bencher" && "$have" == "$FRAME_OMNI_BENCHER_RELEASE_VERSION" ]]; then
    echo "-- frame-omni-bencher $FRAME_OMNI_BENCHER_RELEASE_VERSION already installed"
  else
    install_omni_bencher
  fi
}

ensure_subweight() {
  if command -v subweight >/dev/null 2>&1; then
    echo "-- subweight already installed"
    return
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "::error::cargo is not on PATH, cannot install subweight."
    exit 1
  fi
  echo "-- installing subweight"
  cargo install subweight --locked
}

export_path() {
  echo "$BIN_DIR" >>"${GITHUB_PATH:-/dev/null}"
  export PATH="$BIN_DIR:$PATH"
}

cmd="${1:-ensure}"

case "$cmd" in
ensure)
  mapfile -t missing < <(missing_packages)
  # mapfile keeps one empty element when nothing is missing; drop it.
  [[ ${#missing[@]} -eq 1 && -z "${missing[0]}" ]] && missing=()

  if [[ ${#missing[@]} -gt 0 ]]; then
    apt_install "${missing[@]}"

    mapfile -t still < <(missing_packages)
    [[ ${#still[@]} -eq 1 && -z "${still[0]}" ]] && still=()
    if [[ ${#still[@]} -gt 0 ]]; then
      echo "::error::Still missing after install: ${still[*]}"
      exit 1
    fi
  else
    echo "-- all apt packages already present"
  fi

  ensure_omni_bencher
  ensure_subweight
  export_path
  echo "✅ benchmark runner dependencies are in place"
  ;;

install)
  apt_install "${APT_PACKAGES[@]}"
  if ! command -v cargo >/dev/null 2>&1; then
    echo "!! rustup/cargo is not installed for user $(whoami)."
    echo "!! Install it with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    exit 1
  fi
  cargo install subweight --locked
  install_omni_bencher
  echo
  echo "Make sure $BIN_DIR is on PATH for the runner service user."
  ;;

check)
  mapfile -t missing < <(missing_packages)
  [[ ${#missing[@]} -eq 1 && -z "${missing[0]}" ]] && missing=()

  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "❌ The benchmark runner is missing: ${missing[*]}"
    echo "   Install them with: ./.github/scripts/bench-deps.sh install"
    exit 1
  fi
  echo "-- all apt packages present"

  [[ -x "$BIN_DIR/frame-omni-bencher" ]] || { echo "❌ frame-omni-bencher is not installed"; exit 1; }
  command -v subweight >/dev/null 2>&1 || { echo "❌ subweight is not installed"; exit 1; }
  echo "✅ benchmark runner dependencies are in place"
  ;;

*)
  echo "usage: $0 [ensure|check|install]" >&2
  exit 1
  ;;
esac
