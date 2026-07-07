# Getting Started

This guide walks you through building, configuring, and running trcc-display for the first time.

## Prerequisites

- **Rust 1.82+** ([rustup](https://rustup.rs))
- **Thermalright Digital cooler** (USB VID:PID `0416:8001`)
- **Linux** (Docker works on any platform; native binary requires Linux for libusb)

## Build

```bash
git clone https://github.com/dipenp/trcc-display.git
cd trcc-display
cargo build --release
```

The binary is at `target/release/trcc-display`.

## Verify your device

```bash
# Is the USB device visible?
./target/release/trcc-display detect

# Handshake and identify the cooler model
./target/release/trcc-display probe
```

Expected output:
```
handshake ok: PM=3 SUB=9 (from_cache=false)
→ profile: AX120_DIGITAL (30 LEDs, style ax120)
```

## First run

### With Prometheus

Edit `config/config.json`:

```json
{
    "usb": { "vendor_id": 1046, "product_id": 32769 },
    "profile": { "dir": "config/profiles" },
    "source": { "kind": "prometheus" },
    "prometheus": { "url": "http://localhost:9090" },
    "api": { "enabled": true },
    "render": { "temp_unit": "C" },
    "tiles": [
        {
            "name": "cpu_temp",
            "slot": "main",
            "query": "node_cpu_temp_celsius{cpu='0',core='0'}",
            "unit": "celsius",
            "warn": 75,
            "crit": 85
        }
    ]
}
```

Run:
```bash
./target/release/trcc-display --config config/config.json run
```

### With lm-sensors (fully local)

```bash
# Check available sensors
sensors -j

# Edit config/config.sensors.json with your sensor paths
# Example for AMD CPU:
# "query": "k10temp-pci-00c3/Tctl/temp1_input"

./target/release/trcc-display --config config/config.sensors.json run
```

## Docker

```bash
# Build and run with USB device passthrough
docker compose up -d --build
```

The container maps `/dev/bus/usb` and runs as non-root with the correct udev rules.

## systemd (production)

```bash
# Install the service
sudo cp packaging/systemd/trcc-display.service /etc/systemd/system/
sudo cp packaging/udev/99-thermalright-trcc.rules /etc/udev/rules.d/

# Reload udev to allow non-root USB access
sudo udevadm control --reload
sudo udevadm trigger

# Enable and start
sudo systemctl enable --now trcc-display
```

## Verify it works

```bash
# Check the display is being updated
curl localhost:9110/status | jq

# Override a value temporarily
curl -XPOST localhost:9110/display/value \
  -H 'content-type: application/json' \
  -d '{"slot":"main","value":42,"unit":"celsius","ttl_seconds":30}'

# Live web preview (set "preview_enabled": true in config)
# Open http://localhost:9110/preview in a browser
```

## Next steps

- [Configuration Reference](./config-reference.md) — all config options
- [Device Profiles](./profiles.md) — adding new cooler models
- [REST API Reference](./api-reference.md) — remote control
- [Troubleshooting](./troubleshooting.md) — common issues