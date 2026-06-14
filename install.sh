#!/usr/bin/env bash
# Doser installer.
#
# Safety:
# - Runs with `set -euo pipefail` so any failed step aborts the install.
# - Verifies the downloaded binary against an expected SHA-256 checksum when one
#   is provided (strongly recommended). Set DOSER_SHA256, or place a checksum in
#   ${BASE_URL}/doser_cli.sha256 alongside the binary.
# - Downloads to a temp file and only installs it after verification.
set -euo pipefail

# Configurable download location (override via env). Replace with your real host.
BASE_URL="${DOSER_BASE_URL:-https://yourdomain.com/releases}"
BIN_DEST="${DOSER_BIN_DEST:-/usr/local/bin/doser_cli}"
CONF_DEST="${DOSER_CONF_DEST:-/etc/doser_config.toml}"
CSV_DEST="${DOSER_CSV_DEST:-/etc/doser_config.csv}"

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "$tmpdir"; }
trap cleanup EXIT

echo "Downloading doser_cli from ${BASE_URL} ..."
curl --proto '=https' --tlsv1.2 -fsSL "${BASE_URL}/doser_cli" -o "${tmpdir}/doser_cli"

# Verify checksum. Prefer an explicit DOSER_SHA256; otherwise try a published
# .sha256 file. Abort if neither is available — installing unverified binaries
# that drive hardware is unsafe.
expected_sha="${DOSER_SHA256:-}"
if [ -z "${expected_sha}" ]; then
	if curl --proto '=https' --tlsv1.2 -fsSL "${BASE_URL}/doser_cli.sha256" -o "${tmpdir}/doser_cli.sha256" 2>/dev/null; then
		expected_sha="$(awk '{print $1}' "${tmpdir}/doser_cli.sha256")"
	fi
fi

if [ -z "${expected_sha}" ]; then
	echo "ERROR: no checksum available (set DOSER_SHA256 or publish doser_cli.sha256). Refusing to install unverified binary." >&2
	exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
	actual_sha="$(sha256sum "${tmpdir}/doser_cli" | awk '{print $1}')"
else
	actual_sha="$(shasum -a 256 "${tmpdir}/doser_cli" | awk '{print $1}')"
fi

if [ "${actual_sha}" != "${expected_sha}" ]; then
	echo "ERROR: checksum mismatch for doser_cli" >&2
	echo "  expected: ${expected_sha}" >&2
	echo "  actual:   ${actual_sha}" >&2
	exit 1
fi
echo "Checksum OK."

# Install binary (atomic move into place) and make it executable.
sudo install -m 0755 "${tmpdir}/doser_cli" "${BIN_DEST}"

# Download config files (do not overwrite existing local edits without asking).
for pair in "doser_config.toml:${CONF_DEST}" "doser_config.csv:${CSV_DEST}"; do
	name="${pair%%:*}"
	dest="${pair##*:}"
	if [ -e "${dest}" ]; then
		echo "Keeping existing ${dest} (remove it to refresh)."
		continue
	fi
	curl --proto '=https' --tlsv1.2 -fsSL "${BASE_URL}/${name}" -o "${tmpdir}/${name}"
	sudo install -m 0644 "${tmpdir}/${name}" "${dest}"
done

# Create service user if missing (system user with no shell)
if ! id -u doser >/dev/null 2>&1; then
	sudo useradd --system --create-home --home-dir /var/lib/doser --shell /usr/sbin/nologin doser
fi

# Ensure runtime and log directories
sudo mkdir -p /var/lib/doser
sudo chown -R doser:doser /var/lib/doser
sudo mkdir -p /var/log/doser
sudo chown -R doser:doser /var/log/doser

# Configure logrotate for /var/log/doser/*.log
cat <<'EOF' | sudo tee /etc/logrotate.d/doser >/dev/null
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

# Optionally create a systemd service for auto-start
cat <<EOF | sudo tee /etc/systemd/system/doser.service >/dev/null
[Unit]
Description=Bean Doser Service
After=network.target

[Service]
User=doser
Group=doser
WorkingDirectory=/var/lib/doser
Environment=RUST_LOG=info
ExecStart=${BIN_DEST} --config ${CONF_DEST} --calibration ${CSV_DEST}
Restart=always
RestartSec=1s
# Write logs to dedicated files under /var/log/doser
StandardOutput=append:/var/log/doser/doser.log
StandardError=append:/var/log/doser/doser.err

[Install]
WantedBy=multi-user.target
EOF

# Enable and start the service
sudo systemctl enable doser
sudo systemctl start doser

echo "Doser installed and running!"
