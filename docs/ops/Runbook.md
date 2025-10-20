# Operations Runbook

This runbook covers production service setup, logging, and non-root hardware access for the Doser.

## Systemd service (non-root)

We recommend running the service as a dedicated system user `doser` with no shell. The provided `install.sh` script:

- Creates the `doser` user and home at `/var/lib/doser`.
- Creates `/var/log/doser` and configures log rotation under `/etc/logrotate.d/doser`.
- Installs a systemd unit at `/etc/systemd/system/doser.service` that runs as `User=doser` and writes logs to `/var/log/doser`.
- Configures the service to use both `/etc/doser_config.toml` (main configuration) and `/etc/doser_config.csv` (calibration data).

To (re)load and manage the service:

```sh
sudo systemctl daemon-reload
sudo systemctl enable doser
sudo systemctl start doser
sudo systemctl status doser
```

## Logs and rotation

- stdout → `/var/log/doser/doser.log`
- stderr → `/var/log/doser/doser.err`
- Rotation: weekly, keep 8, compress, copytruncate (see `/etc/logrotate.d/doser`).

To test rotation immediately:

```sh
sudo logrotate -f /etc/logrotate.d/doser
```

## Hardware access without sudo (udev rules)

Grant the `doser` user access to GPIO/I2C devices by adjusting group permissions via udev. Exact groups vary by distro; on Debian/Ubuntu/Raspbian, GPIO/I2C are typically `gpio`, `i2c`.

1. Add the `doser` user to hardware groups:

```sh
sudo usermod -a -G gpio,i2c doser
sudo systemctl restart user@$(id -u doser).service || true
```

2. Create udev rules ensuring device nodes are in the right groups with appropriate modes.

Create `/etc/udev/rules.d/99-doser.rules`:

```udev
# I2C devices owned by group i2c, readable/writeable by group
KERNEL=="i2c-[0-9]*", GROUP="i2c", MODE="0660"

# GPIO character device (newer kernels use /dev/gpiochipN)
KERNEL=="gpiochip[0-9]*", GROUP="gpio", MODE="0660"

# Legacy sysfs export interface (if present)
SUBSYSTEM=="gpio", KERNEL=="gpio*", GROUP="gpio", MODE="0660"
```

Apply the new rules and replug (or reload udev):

```sh
sudo udevadm control --reload-rules
sudo udevadm trigger
```

3. Verify permissions:

```sh
ls -l /dev/i2c-* /dev/gpiochip*
# Expect group i2c/gpio and mode 0660

id doser
# Expect doser : doser gpio i2c
```

If your distro uses different groups (e.g., `dialout`, `plugdev`, or a vendor-specific `spi`), adjust the rules and group memberships accordingly.

## Troubleshooting

- Permission denied opening I2C or GPIO:
  - Confirm `doser` user is in `i2c`/`gpio` groups and udev rules applied (MODE=0660, GROUP correct).
  - Restart service after group changes (`sudo systemctl restart doser`).
- Logs missing or not rotating:
  - Ensure `/var/log/doser` exists and owned by `doser:doser`.
  - Check `/etc/logrotate.d/doser` syntax and run `sudo logrotate -d /etc/logrotate.d/doser` to debug.
- Service fails on boot:
  - `journalctl -u doser -b` for errors.
  - Verify `ExecStart` path and config file exist and are readable by `doser`.
