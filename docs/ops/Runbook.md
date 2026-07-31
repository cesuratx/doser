# Operations Runbook

This runbook covers production service setup, logging, and non-root hardware access for the Doser.

## Systemd service (non-root)

We recommend running the service as a dedicated system user `doser` with no shell. The
provided `install.sh` script (`bash install.sh`):

- Downloads the release tarball for the detected target triple from GitHub Releases
  (`DOSER_REPO`/`DOSER_VERSION`, or a `DOSER_BASE_URL` you supply), verifies its `.sha256`,
  and installs the binary as `/usr/local/bin/doser_cli` (`DOSER_BIN_DEST`).
- Creates the `doser` user and home at `/var/lib/doser`.
- Creates `/var/log/doser` and configures log rotation under `/etc/logrotate.d/doser`.
- Installs a systemd unit at `/etc/systemd/system/doser.service` that runs as `User=doser`,
  is hardened (`NoNewPrivileges`, `ProtectSystem=full`, `ProtectHome`, `PrivateTmp`, …), and
  writes logs to `/var/log/doser`. `PrivateDevices`/`DeviceAllow` are deliberately **not**
  set so `/dev/gpiomem*` stays reachable; `SupplementaryGroups=gpio` is added only when that
  group exists.
- Runs `daemon-reload`, then stops. **It does not enable or start the service** — an
  installer must not bring a network listener up behind your back.

**It does not install config files.** Configs are machine-specific (GPIO pin map, load-cell
gain) and a wrong one drives real hardware, so the script only copies one from a local path
you pass (`DOSER_CONF_SRC`, `DOSER_CALIB_SRC`) and never overwrites an existing file.
Otherwise you place them yourself:

```sh
sudo install -D -m 0644 etc/doser_config.toml /etc/doser_config.toml
sudo install -D -m 0644 doser_config.csv /etc/doser_config.csv   # optional calibration
```

`--calibration /etc/doser_config.csv` is added to `ExecStart` only if that file exists when
the unit is written, so re-run the installer after adding a calibration CSV.

### What the unit actually runs

```
ExecStart=/usr/local/bin/doser_cli --config /etc/doser_config.toml [--calibration …] \
          monitor --bind 127.0.0.1 --port 8080
```

- `monitor` is the only genuinely long-running subcommand (`dose` is one-shot; a unit with no
  subcommand at all makes clap exit 2 and, under `Restart=always`, crash-loop).
- **The monitor UI is unauthenticated and unencrypted**, so the unit binds loopback by
  default. Reach it over an SSH tunnel (`ssh -L 8080:127.0.0.1:8080 pi@doser.local`) or set
  `DOSER_SERVICE_BIND` deliberately before running the installer.
- `Restart=on-failure` with `RestartSec=5s` plus `StartLimitIntervalSec=120` /
  `StartLimitBurst=5` in `[Unit]` — a config error gives up after five tries instead of
  spinning forever.

To enable and manage the service (the installer already ran `daemon-reload`):

```sh
sudo systemctl enable --now doser
sudo systemctl status doser
journalctl -u doser -f

# stop and disable again
sudo systemctl disable --now doser
```

Smoke-test the binary before enabling anything:

```sh
/usr/local/bin/doser_cli --config /etc/doser_config.toml health
```

## Logs and rotation

- stdout → `/var/log/doser/doser.log` — the CLI's own output only (result/status lines).
- stderr → `/var/log/doser/doser.err` — **all log records**, pretty or JSON. This is the file
  to tail when something misbehaves.
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
  - `journalctl -u doser -b` for errors; the log records themselves are in
    `/var/log/doser/doser.err`.
  - Verify the `ExecStart` path (`/usr/local/bin/doser_cli`, **not** `doser`) and that the
    config file exists and is readable by `doser`.
  - Exit code 2 from clap means the argument line is wrong (missing/unknown subcommand or an
    out-of-range value). A missing config or a validation failure exits 1 with the offending
    key named on stderr.
  - After five failed starts in 120 s systemd stops retrying: fix the cause, then
    `sudo systemctl reset-failed doser && sudo systemctl start doser`.
- Monitor UI unreachable from another machine:
  - By design — the unit binds `127.0.0.1`. Tunnel it
    (`ssh -L 8080:127.0.0.1:8080 pi@doser.local`) rather than exposing an unauthenticated UI.
- Tare button returns 403 / 409:
  - `403` means the `X-Doser-Monitor` header is missing (hand-written `curl`), or the request's
    `Host` is not a LAN-looking name.
  - `409` means the scale has not produced a reading yet — check the HX711 wiring.
