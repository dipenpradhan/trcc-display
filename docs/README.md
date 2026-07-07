# trcc-display

Drive **Thermalright "Digital" cooler LED/segment displays** (USB `0416:8001`) from live system metrics.

Supports: Phantom Spirit 120 Digital EVO, AX120/PA120 Digital, Assassin X Digital, and more.

## Quick start

```bash
# Build
cargo build --release

# Check if your device is detected
./target/release/trcc-display detect

# Test the display
./target/release/trcc-display render --slot main --value 57 --unit celsius

# Run with Prometheus + REST API
./target/release/trcc-display --config config/config.json run

# Run headless with lm-sensors (no network, no API)
./target/release/trcc-display --config config/config.sensors.json run
```

## Documentation

| Guide | Description |
|-------|-------------|
| [Getting Started](./getting-started.md) | First-time setup, build, and configuration |
| [Configuration Reference](./config-reference.md) | All config options and tile settings |
| [Device Profiles](./profiles.md) | Adding new cooler models, LED geometry |
| [REST API Reference](./api-reference.md) | All endpoints, request/response formats |
| [CLI Reference](./cli-reference.md) | All subcommands and options |
| [Troubleshooting](./troubleshooting.md) | Common issues and solutions |
| [Architecture](./architecture.md) | Internal design and data flow |

## Supported hardware

| Model | PM byte | LEDs | Slots |
|-------|---------|------|-------|
| AX120 Digital | 1–3 | 30 | 1 (main) |
| PA120 Digital | 16–31 | 84 | 4 (cpu_temp, cpu_use, gpu_temp, gpu_use) |
| PS120 Digital EVO | 48–49 | 93 | 4 (temp, watt, mhz, use) |

Run `trcc-display probe` to identify your device.

## Metric sources

- **Prometheus** — PromQL queries (e.g., `node_cpu_temp`)
- **lm-sensors** — `sensors -j` JSON tree (fully local, no network)

## Features

- 🔌 USB HID control (no kernel driver needed)
- 📊 Live metrics from Prometheus or lm-sensors
- 🌐 REST API for remote control (optional)
- 🔄 Auto-reconnect on USB unplug
- 🔒 No `unsafe` code, strict clippy
- 📦 Docker support, systemd unit included

## License

Apache-2.0. See [LICENSE](../LICENSE).