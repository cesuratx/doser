#!/bin/bash
set -e

# Download binary and config files (replace URLs with your actual release locations)
curl -L https://yourdomain.com/releases/doser_cli -o /usr/local/bin/doser_cli
curl -L https://yourdomain.com/releases/doser_config.toml -o /etc/doser_config.toml
curl -L https://yourdomain.com/releases/doser_config.csv -o /etc/doser_config.csv

# Make binary executable
chmod +x /usr/local/bin/doser_cli

# Optionally create a systemd service for auto-start
cat <<EOF | sudo tee /etc/systemd/system/doser.service
[Unit]
Description=Bean Doser Service
After=network.target

[Service]
ExecStart=/usr/local/bin/doser_cli --config /etc/doser_config.toml
Restart=always
User=root

[Install]
WantedBy=multi-user.target
EOF

# Enable and start the service
sudo systemctl enable doser
sudo systemctl start doser

echo "Doser installed and running!"
