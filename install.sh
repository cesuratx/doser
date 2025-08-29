#!/bin/bash
set -e

# Download binary and config files (replace URLs with your actual release locations)
curl -L https://yourdomain.com/releases/doser_cli -o /usr/local/bin/doser_cli
curl -L https://yourdomain.com/releases/doser_config.toml -o /etc/doser_config.toml
curl -L https://yourdomain.com/releases/doser_config.csv -o /etc/doser_config.csv

# Make binary executable
chmod +x /usr/local/bin/doser_cli

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
cat <<EOF | sudo tee /etc/systemd/system/doser.service
[Unit]
Description=Bean Doser Service
After=network.target

[Service]
User=doser
Group=doser
WorkingDirectory=/var/lib/doser
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/doser_cli --config /etc/doser_config.toml
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
