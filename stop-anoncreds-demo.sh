#!/usr/bin/env bash
#
# stop-anoncreds-demo.sh — teardown for run-anoncreds-demo.sh.
#
# Usage:
#   ./stop-anoncreds-demo.sh                  stop the network, delete the demo artifacts
#   ./stop-anoncreds-demo.sh --keep-artifacts stop the network, keep target/stage3/
#   ./stop-anoncreds-demo.sh --down           DESTROY the network (state, crypto, channel)
#   ./stop-anoncreds-demo.sh --help
#
# The default is a stop, not a destroy: containers and ledger state survive, and the next
# ./run-anoncreds-demo.sh takes its RESUME path (a few seconds). --down runs `fablo down`,
# which removes the generated crypto material and world state, so the next run takes the
# CLEAN path through ./bootstrap.sh and rebuilds everything (several minutes).

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

STAGE3_DIR="$SCRIPT_DIR/target/stage3"
CCAAS_FILTER='name=ccaas_.*_vdrchaincode_'

remove_artifacts=1
destroy=0

while [ $# -gt 0 ]; do
  case "$1" in
    --keep-artifacts) remove_artifacts=0 ;;
    --down|--destroy) destroy=1 ;;
    -h|--help) sed -n '3,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown argument: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

if ! docker info >/dev/null 2>&1; then
  echo "==> Docker is not responding; nothing to stop."
else
  if [ "$destroy" -eq 1 ]; then
    echo "==> Destroying the Fabric network (fablo down)"
    ./fablo down
  else
    # The ccaas_* containers are started with `docker run` by fablo's chaincode install
    # step, not by fabric-docker/docker-compose.yaml, so `fablo stop` (a plain
    # `docker compose stop`) leaves them running. Stop them explicitly, mirroring the
    # explicit `docker start` the runner needs on the resume path.
    ccaas_containers="$(docker ps -q --filter "$CCAAS_FILTER")"
    if [ -n "$ccaas_containers" ]; then
      echo "==> Stopping the CCaaS chaincode containers"
      # shellcheck disable=SC2086 -- the id list is intentionally word-split
      docker stop $ccaas_containers >/dev/null
    fi
    echo "==> Stopping the Fabric network (fablo stop)"
    ./fablo stop
  fi
fi

if [ "$remove_artifacts" -eq 1 ]; then
  case "$STAGE3_DIR" in
    */target/stage3)
      rm -rf "$STAGE3_DIR"
      echo "==> Removed $STAGE3_DIR"
      ;;
    *)
      echo "Refusing to delete unexpected artifact path: $STAGE3_DIR" >&2
      exit 1
      ;;
  esac
fi

if [ "$destroy" -eq 1 ]; then
  echo "==> Done. The next ./run-anoncreds-demo.sh will rebuild the network from scratch."
else
  echo "==> Done. The next ./run-anoncreds-demo.sh will resume this network in seconds."
fi
