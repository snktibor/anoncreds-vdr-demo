#!/usr/bin/env bash
#
# run-anoncreds-demo.sh — the AnonCreds v2 interop demo, end to end, in one command.
#
# Watch a real credx (Rust) credential definition get published to a live Hyperledger
# Fabric VDR, a holder get revoked on-ledger, and the ledger's own answer drive the
# accept/reject decision:
#
#   network up -> objects published -> credentials issued -> holder revoked on-ledger
#   -> revoked holder rejected -> refreshed holder accepted
#
# The demo spans two languages and two repositories, so it runs in three legs:
#
#   1. anoncreds-vdr-demo (Rust)   `cargo run -- emit`     credx crypto -> bundle.json
#   2. fabric-vdr-core    (Java)   AnonCredsDemoIT         bundle.json -> live VDR
#                                                          -> vdr-report.json
#   3. anoncreds-vdr-demo (Rust)   `cargo run -- verify`   vdr-report.json -> reassembled
#                                                          IssuerPublic, real presentations
#
# This repository carries the Rust demo AND the network recipe (fablo, fablo-config.json,
# bootstrap.sh), so the only thing it needs from outside is the chaincode. Clone that
# repository into this directory, named exactly fabric-vdr-core, before the first run:
#
#   git clone https://github.com/snktibor/fabric-vdr-core.git fabric-vdr-core
#
# The name and the location both matter: fablo-config.json deploys the chaincode from
# ./fabric-vdr-core, resolved by Fablo against the directory holding fablo-config.json.
#
# See docs/anoncreds-demo.md for the walkthrough, the prerequisites and what each step
# is supposed to print.
#
# Usage:
#   ./run-anoncreds-demo.sh            # bring the network up if needed, then run the demo
#   ./run-anoncreds-demo.sh --help
#
# Teardown: ./stop-anoncreds-demo.sh

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# The Rust demo is this repository, so DEMO_DIR is the script's own directory. The
# chaincode is a separate repository cloned in here; the network state is generated here.
DEMO_DIR="$SCRIPT_DIR"
CHAINCODE_DIR="$SCRIPT_DIR/fabric-vdr-core"
STAGE3_DIR="$DEMO_DIR/target/stage3"
CRYPTO_CONFIG="$SCRIPT_DIR/fablo-target/fabric-config/crypto-config"

# fabric-vdr-core resolves its two out-of-repo paths as system property -> environment
# variable -> built-in default, and those defaults still assume the older layout where
# this demo sat beside the network rather than holding it. Name both explicitly here
# rather than editing the chaincode repository. They are defaulted, not forced: an
# override already in the environment still wins, which keeps the failure-injection
# recipe in docs/anoncreds-demo.md section 6 working.
export VDR_DEMO_STAGE3_DIR="${VDR_DEMO_STAGE3_DIR:-$STAGE3_DIR}"
export VDR_E2E_CRYPTO_PATH="${VDR_E2E_CRYPTO_PATH:-$CRYPTO_CONFIG/peerOrganizations/org1.example.com}"

# Only the peer0 CCaaS container matters for the harness (E2EConfig points the Gateway
# at localhost:7041), but both are started so peer1 can endorse too.
CCAAS_FILTER='name=ccaas_.*_vdrchaincode_'

# How long to wait for the chaincode to answer a read-only query after a bring-up.
READY_TIMEOUT_SECONDS=180

# Named so the failure trap can say which leg died.
STEP="startup"

if [ -t 1 ]; then
  BOLD=$'\033[1m'; RED=$'\033[31m'; GREEN=$'\033[32m'; DIM=$'\033[2m'; OFF=$'\033[0m'
else
  BOLD=""; RED=""; GREEN=""; DIM=""; OFF=""
fi

banner() { printf '\n%s==> %s%s\n' "$BOLD" "$*" "$OFF"; }
note()   { printf '%s    %s%s\n' "$DIM" "$*" "$OFF"; }

# Every leg of this demo has a way of failing quietly, so the script is deliberately
# paranoid: `set -e` plus this trap means the first non-zero exit anywhere — including
# inside a function or a subshell, hence `set -E` — stops the run and propagates.
on_error() {
  local status=$?
  printf '\n%s!! FAILED: %s (exit status %d)%s\n' "$RED$BOLD" "$STEP" "$status" "$OFF"
  printf '%s!! The demo did NOT complete. Do not read anything above as a pass.%s\n' "$RED" "$OFF"
  exit "$status"
}
trap on_error ERR

usage() {
  sed -n '3,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
  -h|--help|help) usage; exit 0 ;;
  "") ;;
  *) echo "Unknown argument: $1 (try --help)" >&2; exit 2 ;;
esac

# ---------------------------------------------------------------------------
# Step 0 — preflight
#
# Everything checked here has bitten this demo at least once. Failing now with a
# sentence beats failing forty minutes in with a stack trace.
# ---------------------------------------------------------------------------
STEP="preflight checks"
banner "[0/4] Preflight"

require_dir() {
  if [ ! -d "$1" ]; then
    echo "Missing $1" >&2
    echo "$2" >&2
    exit 1
  fi
}

require_dir "$CHAINCODE_DIR" \
  "Clone the chaincode repository into this directory, named exactly fabric-vdr-core:
    git clone https://github.com/snktibor/fabric-vdr-core.git fabric-vdr-core"

for tool in docker mvn java; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is not on PATH." >&2; exit 1; }
done

# rustup installs cargo into ~/.cargo/bin, which is not always on a non-login PATH.
if command -v cargo >/dev/null 2>&1; then
  CARGO="$(command -v cargo)"
elif [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" ]; then
  CARGO="${CARGO_HOME:-$HOME/.cargo}/bin/cargo"
else
  echo "cargo is not on PATH and not at ~/.cargo/bin/cargo — install Rust (https://rustup.rs)." >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "Docker is not responding. Start Docker Desktop or OrbStack and re-run." >&2
  exit 1
fi

# JDK 17 exactly. JaCoCo 0.8.12 is bound to prepare-agent in fabric-vdr-core/pom.xml and
# sets ${argLine} for the forked JVM, so a newer JDK dies with "Unsupported class file
# major version" before the integration test gets a chance to run. Ask Maven which JVM it
# will actually use rather than trusting `java` on PATH.
MAVEN_JAVA="$(mvn -v 2>/dev/null | sed -n 's/^Java version: \([0-9]*\).*/\1/p' | head -1)"
if [ "$MAVEN_JAVA" != "17" ]; then
  echo "Maven is running on JDK ${MAVEN_JAVA:-unknown}; this build needs JDK 17." >&2
  echo "  export JAVA_HOME=\"\$(/usr/libexec/java_home -v 17)\"   # macOS" >&2
  echo "  export PATH=\"\$JAVA_HOME/bin:\$PATH\"" >&2
  exit 1
fi

note "cargo:  $("$CARGO" --version)"
note "maven:  $(mvn -v 2>/dev/null | head -1)"
note "jdk:    $MAVEN_JAVA (Maven's JVM)"
note "docker: $(docker version --format '{{.Server.Version}}' 2>/dev/null)"

# Start from a clean slate so `verify` can never be fed a stale vdr-report.json from an
# earlier, half-finished run. Everything under target/ is gitignored and regenerated.
case "$STAGE3_DIR" in
  */target/stage3) rm -rf "$STAGE3_DIR" ;;
  *) echo "Refusing to delete unexpected artifact path: $STAGE3_DIR" >&2; exit 1 ;;
esac
mkdir -p "$STAGE3_DIR"
note "artifact directory reset: $STAGE3_DIR"

# ---------------------------------------------------------------------------
# Step 1 — the Fabric network
#
# Two cases, and they need different commands.
#
#   CLEAN  — no fablo-target/fabric-config/crypto-config. `./bootstrap.sh` builds the
#            two Docker images and runs `fablo up`, which generates crypto material,
#            creates the channel and deploys the CCaaS chaincode. Several minutes.
#
#   RESUME — crypto-config is there, so the network was generated before and is merely
#            stopped (a `fablo stop`, a Docker restart, a reboot). Running bootstrap.sh
#            here FAILS: `fablo up` -> generateArtifacts -> certsGenerate refuses with
#            "Can't generate certs, directory already exists" rather than clobber the
#            identities the ledger already trusts. The recovery is `./fablo start`, plus
#            an explicit `docker start` for the two ccaas_* containers — those are
#            launched with `docker run` by fablo's chaincode install step, they are not
#            in fabric-docker/docker-compose.yaml, so `docker compose up -d` (which is
#            all `fablo start` does) leaves them stopped and every transaction fails to
#            endorse.
#
# Which case applies is decided by crypto-config alone, and that is exact: `fablo down`
# removes it (commands-generated.sh networkDown), `fablo stop` does not.
# ---------------------------------------------------------------------------
STEP="bringing the Fabric network up"

# Read-only, argument-free chaincode query. Succeeding proves the whole path the harness
# needs: peer up, channel joined, chaincode committed, CCaaS container connected back,
# contract routing alive. It writes nothing to world state.
network_ready() {
  ./fablo chaincode query \
    peer0.org1.example.com my-channel1 vdrchaincode \
    '{"Args":["VdrContract:listSchemas"]}' >/dev/null 2>&1
}

banner "[1/4] Fabric network"
if [ -d "$CRYPTO_CONFIG" ]; then
  note "existing network detected (crypto material present) — resuming, not regenerating"
  ./fablo start
  ccaas_containers="$(docker ps -aq --filter "$CCAAS_FILTER")"
  if [ -n "$ccaas_containers" ]; then
    note "starting the CCaaS chaincode containers (fablo start does not)"
    # shellcheck disable=SC2086 -- the id list is intentionally word-split
    docker start $ccaas_containers >/dev/null
  else
    note "no ccaas_* containers found; the readiness probe below will decide"
  fi
else
  note "no network state found — clean bring-up via ./bootstrap.sh (builds images, several minutes)"
  ./bootstrap.sh
fi

printf '    waiting for the chaincode to answer'
deadline=$(( SECONDS + READY_TIMEOUT_SECONDS ))
until network_ready; do
  if [ "$SECONDS" -ge "$deadline" ]; then
    printf '\n'
    echo "The chaincode did not answer within ${READY_TIMEOUT_SECONDS}s." >&2
    echo "Check:  docker ps -a | grep -E 'peer0|ccaas'" >&2
    echo "        docker logs \$(docker ps -aq --filter '$CCAAS_FILTER' | head -1)" >&2
    echo "If the network state is damaged, rebuild it from scratch:  ./bootstrap.sh reset" >&2
    exit 1
  fi
  printf '.'
  sleep 3
done
printf '\n'
note "peer0.org1.example.com:7041 is serving VdrContract on my-channel1"

# ---------------------------------------------------------------------------
# Step 2 — credx produces the material (Rust, off-ledger)
# ---------------------------------------------------------------------------
STEP="emitting the credx artifacts (cargo run -- emit)"
banner "[2/4] credx: issue, revoke, refresh — and write the wire bundle"
note "the first build compiles credx from git and takes a few minutes"
( cd "$DEMO_DIR" && "$CARGO" run -- emit )

[ -f "$STAGE3_DIR/bundle.json" ]  || { echo "emit produced no bundle.json" >&2; exit 1; }
[ -f "$STAGE3_DIR/session.json" ] || { echo "emit produced no session.json" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Step 3 — the Java harness drives the live VDR
#
# The isolation flags are load-bearing. DO NOT "simplify" them to -DskipTests.
#
#   -Dit.test=AnonCredsDemoIT              overrides failsafe's default includes, so
#                                          RunCucumberIT's 20 scenarios do not also run.
#   -Dtest=none                            surefire runs no unit tests.
#   -Dsurefire.failIfNoSpecifiedTests=false  ...and does not fail for having run none.
#
# `-DskipTests` looks like the obvious way to silence the unit tests and is a trap:
# maven-failsafe-plugin binds its own skipTests parameter to the SAME ${skipTests}
# property surefire uses, so `mvn verify -Dit.test=AnonCredsDemoIT -DskipTests` skips the
# integration test too. The build goes green having executed nothing at all — no
# transactions, no ledger writes, no vdr-report.json. This cost real debugging time once.
#
# The inverse guard is free and worth knowing: failsafe's failIfNoSpecifiedTests defaults
# to true, so if -Dit.test ever stops matching (a rename, a typo) the build fails loudly
# instead of silently running zero integration tests.
#
# The two paths the harness needs — the crypto material and this run's artifact directory —
# arrive as VDR_E2E_CRYPTO_PATH and VDR_DEMO_STAGE3_DIR, exported at the top of this script
# and inherited by Maven's forked test JVM. Nothing in fabric-vdr-core has to change for
# this layout.
# ---------------------------------------------------------------------------
STEP="publishing to the VDR (AnonCredsDemoIT)"
banner "[3/4] Java harness: publish to the live VDR and read it back"
(
  cd "$CHAINCODE_DIR" && mvn verify \
    -Dit.test=AnonCredsDemoIT \
    -Dtest=none \
    -Dsurefire.failIfNoSpecifiedTests=false
)

# Independent evidence that the harness actually ran, not merely that Maven exited 0.
# A false-green Maven invocation (see above) leaves no report behind, and a report older
# than the bundle is a leftover from a previous run.
if [ ! -f "$STAGE3_DIR/vdr-report.json" ]; then
  echo "Maven exited 0 but wrote no vdr-report.json — the integration test did not run." >&2
  exit 1
fi
if [ ! "$STAGE3_DIR/vdr-report.json" -nt "$STAGE3_DIR/bundle.json" ]; then
  echo "vdr-report.json is older than bundle.json — it is stale, the harness did not run." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 4 — credx verifies against what the ledger returned
# ---------------------------------------------------------------------------
STEP="verifying the ledger's answer (cargo run -- verify)"
banner "[4/4] credx: reassemble IssuerPublic from the ledger and verify presentations"
( cd "$DEMO_DIR" && "$CARGO" run -- verify )

STEP="done"
trap - ERR

cat <<EOF

${GREEN}${BOLD}==> DEMO PASSED — AnonCreds v2 revocation round-tripped through a live Fabric VDR${OFF}

    schema, credential definition and revocation registry definition published on-ledger
    genesis accumulator written        -> revocation registry entry version 0
    Alice revoked off-ledger, published -> revocation registry entry version 1
    IssuerPublic reassembled from getCredDef + getRevRegDef + getRevRegEntry (spec 8.1.5)
    Bob accepted at v0 (genesis witness) and at v1 (refreshed witness)
    revoked Alice REJECTED against the ledger's current accumulator

    artifacts: $STAGE3_DIR
      bundle.json      what credx published
      vdr-report.json  what the ledger returned
      session.json     the credx state the verification is judged against

    teardown: ./stop-anoncreds-demo.sh
EOF
