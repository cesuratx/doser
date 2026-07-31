#!/usr/bin/env bash
# Doser installer.
#
# What this does:
#   1. Maps this machine's OS/arch to a published release target triple.
#   2. Downloads doser-<target>.tar.gz (+ .sha256) from GitHub Releases,
#      verifies the checksum, extracts, and installs the binary.
#   3. Installs a config/calibration file ONLY if you point it at one; it never
#      overwrites an existing one.
#   4. On Linux, writes a systemd unit and leaves it DISABLED, then prints the
#      commands to enable it. Installing something must not start it.
#
# Integrity vs authenticity -- read this before trusting the checksum:
#   The .sha256 published next to the tarball is served from the SAME origin as
#   the tarball. It therefore only proves the download was not corrupted in
#   transit; it proves NOTHING about authenticity, because anyone able to serve
#   you a malicious tarball can serve you a matching .sha256. For an actual
#   trust decision, obtain the checksum out of band (release notes read in a
#   browser, a signed manifest, a colleague) and pass it explicitly:
#       DOSER_SHA256=<64-hex-digits> bash install.sh
#
# Environment overrides:
#   DOSER_REPO      GitHub "owner/repo" to install from (default cesuratx/doser)
#   DOSER_VERSION   Release tag, e.g. v0.3.0, or "latest" (default latest)
#   DOSER_BASE_URL  Full base URL holding the release assets; bypasses the two
#                   above. There is no default -- an unset/empty value falls
#                   back to DOSER_REPO, and if that is empty too the script
#                   aborts rather than guessing a download origin.
#   DOSER_SHA256    Expected sha256 of the tarball, obtained out of band.
#   DOSER_TARGET    Force a release target triple instead of autodetecting.
#   DOSER_CONF_SRC  Local path to a doser_config.toml to install.
#   DOSER_CALIB_SRC Local path to a calibration CSV (raw,grams) to install.
set -euo pipefail

DOSER_REPO="${DOSER_REPO:-cesuratx/doser}"
DOSER_VERSION="${DOSER_VERSION:-latest}"
BIN_DEST="${DOSER_BIN_DEST:-/usr/local/bin/doser_cli}"
CONF_DEST="${DOSER_CONF_DEST:-/etc/doser_config.toml}"
CSV_DEST="${DOSER_CSV_DEST:-/etc/doser_config.csv}"
SERVICE_BIND="${DOSER_SERVICE_BIND:-127.0.0.1}"
SERVICE_PORT="${DOSER_SERVICE_PORT:-8080}"

# Run privileged steps through sudo unless we are already root (containers and
# minimal images often have no sudo at all).
if [ "$(id -u)" -eq 0 ]; then
  SUDO=""
elif command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
else
  echo "ERROR: this installer needs root privileges and sudo is not available." >&2
  echo "Re-run as root, or set DOSER_BIN_DEST/DOSER_CONF_DEST to writable paths." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Resolve the download origin. No placeholder default: an unconfigured source
# must fail loudly rather than silently sudo-installing a binary from a host
# nobody in this project controls.
# ---------------------------------------------------------------------------
BASE_URL="${DOSER_BASE_URL:-}"
if [ -z "${BASE_URL}" ]; then
  if [ -z "${DOSER_REPO}" ]; then
    echo "ERROR: no download source configured." >&2
    echo "Set DOSER_REPO=owner/repo (GitHub Releases) or DOSER_BASE_URL=https://..." >&2
    echo "to the location holding doser-<target>.tar.gz, then re-run." >&2
    exit 1
  fi
  if [ "${DOSER_VERSION}" = "latest" ]; then
    BASE_URL="https://github.com/${DOSER_REPO}/releases/latest/download"
  else
    # Accept both "0.3.0" and "v0.3.0"; release tags are v-prefixed.
    case "${DOSER_VERSION}" in
      v*) tag="${DOSER_VERSION}" ;;
      *) tag="v${DOSER_VERSION}" ;;
    esac
    BASE_URL="https://github.com/${DOSER_REPO}/releases/download/${tag}"
  fi
fi

# ---------------------------------------------------------------------------
# Pick the right artifact for this machine. The release workflow publishes
# exactly these four target triples; anything else must not be installed.
# ---------------------------------------------------------------------------
target="${DOSER_TARGET:-}"
if [ -z "${target}" ]; then
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Linux:aarch64 | Linux:arm64) target="aarch64-unknown-linux-gnu" ;;
    Linux:x86_64 | Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
    Darwin:arm64 | Darwin:aarch64) target="aarch64-apple-darwin" ;;
    Darwin:x86_64) target="x86_64-apple-darwin" ;;
    Linux:armv6l | Linux:armv7l)
      echo "ERROR: 32-bit ARM (${arch}) has no published release artifact." >&2
      echo "Install 64-bit Raspberry Pi OS, or build from source:" >&2
      echo "  cargo build --release -p doser_cli --features hardware" >&2
      exit 1
      ;;
    *)
      echo "ERROR: unsupported platform ${os}/${arch}; no release artifact for it." >&2
      echo "Set DOSER_TARGET to a published target triple to override." >&2
      exit 1
      ;;
  esac
fi

tarball="doser-${target}.tar.gz"

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "${tmpdir}"; }
trap cleanup EXIT

echo "Installing ${tarball} from ${BASE_URL} ..."
curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "${BASE_URL}/${tarball}" \
  -o "${tmpdir}/${tarball}"

# Prefer an explicitly supplied checksum; fall back to the published one (see
# the integrity-vs-authenticity note at the top of this file). Abort if neither
# is available -- installing an unverified binary that drives a motor is unsafe.
expected_sha="${DOSER_SHA256:-}"
if [ -z "${expected_sha}" ]; then
  if curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "${BASE_URL}/${tarball}.sha256" \
    -o "${tmpdir}/${tarball}.sha256" 2>/dev/null; then
    expected_sha="$(awk 'NR==1{print $1}' "${tmpdir}/${tarball}.sha256")"
    echo "Note: using same-origin ${tarball}.sha256 (integrity only, not authenticity)."
  fi
fi

if [ -z "${expected_sha}" ]; then
  echo "ERROR: no checksum available for ${tarball}." >&2
  echo "Set DOSER_SHA256=<sha256> (recommended) or publish ${tarball}.sha256." >&2
  echo "Refusing to install an unverified binary." >&2
  exit 1
fi

if [[ ! "${expected_sha}" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "ERROR: expected checksum is not 64 hex digits: '${expected_sha}'" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha="$(sha256sum "${tmpdir}/${tarball}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_sha="$(shasum -a 256 "${tmpdir}/${tarball}" | awk '{print $1}')"
else
  echo "ERROR: neither sha256sum nor shasum is available; cannot verify download." >&2
  exit 1
fi

# Case-insensitive compare so a hand-pasted uppercase DOSER_SHA256 still works.
if [ "$(printf '%s' "${actual_sha}" | tr 'A-F' 'a-f')" \
  != "$(printf '%s' "${expected_sha}" | tr 'A-F' 'a-f')" ]; then
  echo "ERROR: checksum mismatch for ${tarball}" >&2
  echo "  expected: ${expected_sha}" >&2
  echo "  actual:   ${actual_sha}" >&2
  exit 1
fi
echo "Checksum OK."

# The release tarball contains a single bare `doser_cli` binary.
mkdir -p "${tmpdir}/unpack"
tar -xzf "${tmpdir}/${tarball}" -C "${tmpdir}/unpack"
if [ ! -f "${tmpdir}/unpack/doser_cli" ]; then
  echo "ERROR: ${tarball} does not contain a doser_cli binary." >&2
  exit 1
fi

${SUDO} install -d -m 0755 "$(dirname "${BIN_DEST}")"
${SUDO} install -m 0755 "${tmpdir}/unpack/doser_cli" "${BIN_DEST}"
echo "Installed ${BIN_DEST}"

# ---------------------------------------------------------------------------
# Config + calibration.
#
# These are deliberately NOT downloaded. Both are machine-specific (GPIO pin
# map, load-cell gain) and a wrong one drives real hardware, so shipping a
# generic remote default would be actively dangerous -- and it would add a
# second network origin to trust. Point DOSER_CONF_SRC / DOSER_CALIB_SRC at a
# local file (e.g. etc/doser_config.toml from a repo checkout) to install one.
# An existing file is never clobbered.
# ---------------------------------------------------------------------------
install_if_absent() {
  local src="$1"
  local dest="$2"
  local label="$3"
  if [ -z "${src}" ]; then
    return 0
  fi
  if [ ! -f "${src}" ]; then
    echo "ERROR: ${label} source not found: ${src}" >&2
    exit 1
  fi
  if [ -e "${dest}" ]; then
    echo "Keeping existing ${dest} (remove it to refresh)."
    return 0
  fi
  ${SUDO} install -d -m 0755 "$(dirname "${dest}")"
  ${SUDO} install -m 0644 "${src}" "${dest}"
  echo "Installed ${label} -> ${dest}"
}

install_if_absent "${DOSER_CONF_SRC:-}" "${CONF_DEST}" "config"
install_if_absent "${DOSER_CALIB_SRC:-}" "${CSV_DEST}" "calibration"

if [ ! -e "${CONF_DEST}" ]; then
  echo "No config at ${CONF_DEST}."
  echo "  Copy etc/doser_config.toml from the repo and adjust the [pins] section, then:"
  echo "  sudo install -D -m 0644 doser_config.toml ${CONF_DEST}"
fi

# Everything below is systemd/Linux only; on macOS the binary install is the
# whole job.
if [ "$(uname -s)" != "Linux" ] || ! command -v systemctl >/dev/null 2>&1; then
  echo "Done. (No systemd here -- skipping service setup.)"
  echo "Try: ${BIN_DEST} --config ${CONF_DEST} health"
  exit 0
fi

# Create service user if missing (system user with no shell)
if ! id -u doser >/dev/null 2>&1; then
  ${SUDO} useradd --system --create-home --home-dir /var/lib/doser \
    --shell /usr/sbin/nologin doser
fi

# GPIO access comes from group membership on /dev/gpiomem*, not from root.
if getent group gpio >/dev/null 2>&1; then
  ${SUDO} usermod -aG gpio doser
  gpio_group_line="SupplementaryGroups=gpio"
else
  echo "WARNING: no 'gpio' group on this system; the service will not be able to" >&2
  echo "         reach /dev/gpiomem*. Create it (or install Raspberry Pi OS) and" >&2
  echo "         re-run this script before enabling the service." >&2
  gpio_group_line="# SupplementaryGroups=gpio  # no 'gpio' group at install time"
fi

# Ensure runtime and log directories
${SUDO} mkdir -p /var/lib/doser
${SUDO} chown -R doser:doser /var/lib/doser
${SUDO} mkdir -p /var/log/doser
${SUDO} chown -R doser:doser /var/log/doser

# Configure logrotate for /var/log/doser/*.log
cat <<'EOF' | ${SUDO} tee /etc/logrotate.d/doser >/dev/null
/var/log/doser/*.log {
	weekly
	rotate 8
	missingok
	notifempty
	compress
	delaycompress
	copytruncate
	create 0640 doser doser
}
EOF

# The unit runs `monitor`, the only long-running subcommand. `dose` is one-shot
# by nature, so an always-on service could never have run it -- and a unit with
# no subcommand at all makes clap exit 2 immediately, which under Restart=always
# is just a crash loop.
#
# `--calibration` is only passed when the file exists: pointing it at a missing
# path is a hard error. Re-run this script after installing a calibration CSV.
calib_arg=""
if [ -e "${CSV_DEST}" ]; then
  calib_arg=" --calibration ${CSV_DEST}"
fi

# Bind to loopback by default: the monitor UI is unauthenticated, so exposing
# it on the LAN would hand anyone on the network a live view of the machine.
# Reach it over an SSH tunnel, or set DOSER_SERVICE_BIND deliberately.
cat <<EOF | ${SUDO} tee /etc/systemd/system/doser.service >/dev/null
[Unit]
Description=Doser live weight monitor
Documentation=https://github.com/${DOSER_REPO}
After=network-online.target
Wants=network-online.target
# A config/usage error is not worth retrying forever; give up after 5 tries.
# (These live in [Unit] since systemd v229.)
StartLimitIntervalSec=120
StartLimitBurst=5

[Service]
Type=simple
User=doser
Group=doser
${gpio_group_line}
WorkingDirectory=/var/lib/doser
Environment=RUST_LOG=info
ExecStart=${BIN_DEST} --config ${CONF_DEST}${calib_arg} monitor --bind ${SERVICE_BIND} --port ${SERVICE_PORT}
Restart=on-failure
RestartSec=5s

# Hardening. Deliberately NOT set: PrivateDevices/DeviceAllow, which would hide
# /dev/gpiomem* and /dev/gpiochip* and break the HX711 and stepper drivers.
NoNewPrivileges=yes
ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
ProtectControlGroups=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
RestrictSUIDSGID=yes
LockPersonality=yes

# Write logs to dedicated files under /var/log/doser
StandardOutput=append:/var/log/doser/doser.log
StandardError=append:/var/log/doser/doser.err

[Install]
WantedBy=multi-user.target
EOF

${SUDO} systemctl daemon-reload

# Installed, NOT enabled and NOT started. An install step must not bring a
# network service up on the operator's machine behind their back.
cat <<EOF

Doser installed.

  binary:  ${BIN_DEST}
  config:  ${CONF_DEST}
  service: /etc/systemd/system/doser.service (installed, NOT enabled)

Smoke-test it first:
  ${BIN_DEST} --config ${CONF_DEST} health

The service is not running. To start the monitor UI on ${SERVICE_BIND}:${SERVICE_PORT}:
  sudo systemctl enable --now doser
  systemctl status doser
  journalctl -u doser -f

To stop and disable it again:
  sudo systemctl disable --now doser
EOF
