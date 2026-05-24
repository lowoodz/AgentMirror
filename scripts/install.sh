#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

PREFIX="${SMR_INSTALL_PREFIX:-${HOME}/.local}"
BINDIR="${PREFIX}/bin"
CONFDIR="${PREFIX}/etc/securemodelroute"

echo "==> Building release..."
cargo build --release

echo "==> Installing to ${PREFIX}"
mkdir -p "${BINDIR}" "${CONFDIR}"
install -m 755 "${ROOT}/target/release/smr" "${BINDIR}/smr"

if [[ ! -f "${CONFDIR}/smr.yaml" ]]; then
  install -m 644 "${ROOT}/config/smr.example.yaml" "${CONFDIR}/smr.yaml"
  echo "    Created ${CONFDIR}/smr.yaml"
fi

LAUNCHER="${BINDIR}/securemodelroute"
cat > "${LAUNCHER}" << EOF
#!/usr/bin/env bash
exec "${BINDIR}/smr" --config "${CONFDIR}/smr.yaml" --open "\$@"
EOF
chmod +x "${LAUNCHER}"

echo ""
echo "Installed:"
echo "  binary:   ${BINDIR}/smr"
echo "  launcher: ${LAUNCHER}  (opens GUI)"
echo "  config:   ${CONFDIR}/smr.yaml"
echo ""
echo "Run:  securemodelroute"
echo "Or:   smr --config ${CONFDIR}/smr.yaml --open"
echo ""
if [[ ":${PATH}:" != *":${BINDIR}:"* ]]; then
  echo "Add to PATH:  export PATH=\"${BINDIR}:\$PATH\""
fi
