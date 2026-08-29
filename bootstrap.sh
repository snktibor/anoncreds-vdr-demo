#!/usr/bin/env bash
# One-command bring-up: builds the chaincode image, then starts (or restarts) the
# Fablo network.
#
# Usage:
#   ./bootstrap.sh            # build + fablo up
#   ./bootstrap.sh reset      # build + fablo reset (full restart)
#   ./bootstrap.sh prune      # fablo prune + generate + build + up (clean slate)
#   ./bootstrap.sh build      # build the chaincode image only
#   ./bootstrap.sh update     # rebuild the chaincode and upgrade it on a running network
#
# Normally you do not call this directly — ./run-anoncreds-demo.sh runs it for you when
# there is no network yet.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

CHAINCODE_TAG="vdr-chaincode:0.0.7"

build_chaincode() {
  echo "==> Building the chaincode image ($CHAINCODE_TAG)"
  if [ ! -d fabric-vdr-core ]; then
    echo "ERROR: fabric-vdr-core/ not found. Clone the chaincode repository in here:"
    echo "  git clone -b feat/revocation-registry https://github.com/snktibor/fabric-vdr-core.git fabric-vdr-core"
    exit 1
  fi
  (cd fabric-vdr-core && mvn clean package -DskipTests)
  docker build -t "$CHAINCODE_TAG" fabric-vdr-core/
}

update_chaincode() {
  echo "==> Upgrading the chaincode on the running network ($CHAINCODE_TAG)"
  build_chaincode

  # The lifecycle version MUST be unique on every repeated update. Passing the same
  # version (say "0.0.7") every time lets fablo's approve step pick an arbitrary package
  # out of the peer's "queryinstalled" response among those sharing a label — not
  # necessarily the one installed last — because Fabric cannot delete or overwrite a
  # package under an identical label. That previously produced a package-id divergence
  # between peer0 and peer1 and a "peer will not accept external chaincode connection"
  # failure.
  local cc_version="0.0.7-$(date +%Y%m%d%H%M%S)"
  echo "==> New lifecycle version: $cc_version"

  echo "==> Removing the CCaaS containers so they restart from the new image"
  # Fablo bakes the chaincode version from fablo-config.json into the container names
  # (independently of the lifecycle version passed to the upgrade), so the names are
  # collected by pattern rather than hardcoded — this keeps working if the version in
  # the config changes.
  local ccaas_containers
  ccaas_containers="$(docker ps -aq --filter 'name=ccaas_.*_vdrchaincode_')"
  if [ -n "$ccaas_containers" ]; then
    # shellcheck disable=SC2086 -- the id list is intentionally word-split
    docker rm -f $ccaas_containers >/dev/null
  fi
  ./fablo chaincode upgrade vdrchaincode "$cc_version"
}

cmd="${1:-up}"
case "$cmd" in
  build)
    build_chaincode
    ;;
  reset)
    build_chaincode
    ./fablo reset
    ;;
  up|"")
    build_chaincode
    ./fablo up
    ;;
  prune)
    ./fablo prune
    ./fablo generate
    build_chaincode
    ./fablo up
    ;;
  update)
    update_chaincode
    ;;
  *)
    echo "Unknown command: $cmd (use: build | up | reset | update | prune)"
    exit 1
    ;;
esac

echo "==> Done."
