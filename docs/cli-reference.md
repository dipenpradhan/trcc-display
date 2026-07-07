# CLI Reference

```
trcc-display [--config <path>] [-v|-vv] <command>

Commands:
  run            Run the driver (default): refresh, render, serve REST.
  detect         List connected devices matching the USB VID/PID.
  probe          Handshake and print identity + resolved profile.
  once           Fetch metrics once, render a single frame, exit.
  render         Render one explicit value to a slot, exit.
  test-pattern   Map the LEDs: walk one at a time or light all.

Options:
  -c, --config <path>    Path to config JSON (default: config.json)
  -v                     Debug logging
  -vv                    Trace logging
  RUST_LOG=<filter>      Override log level (e.g., RUST_LOG=trcc_display=debug)
```

## run

Start the driver. Refreshes metrics on cadence, renders frames, serves REST API (if enabled).

```bash
trcc-display --config config.json run
trcc-display run  # default command
```

## detect

List USB devices matching the configured VID/PID.

```bash
trcc-display detect
# → found 1 device(s):
# →   bus 001 device 042: 0416:8001
```

## probe

Handshake with the device and print its identity.

```bash
trcc-display probe
# → handshake ok: PM=3 SUB=9 (from_cache=false)
# → → profile: AX120_DIGITAL (30 LEDs, style ax120)
```

The handshake result is cached (see `state/probe_cache.json`) because the firmware only answers once per power cycle.

## once

Fetch metrics once, render a single frame, and exit.

```bash
trcc-display once
# → cpu_temp    = 57.0
# → gpu_temp    = 72.5
# → sent one frame to AX120_DIGITAL
```

## render

Render one explicit value to a slot and exit. No metric source needed.

```bash
trcc-display render --slot main --value 57 --unit celsius
trcc-display render --slot gpu_temp --value 85 --unit celsius --color 255,60,60
```

Options:
| Flag | Default | Description |
|------|---------|-------------|
| `--slot` | — | Target slot name |
| `--value` | — | Number to display |
| `--unit` | — | Unit marker (`celsius`, `fahrenheit`, `percent`) |
| `--color` | `0,200,120` | RGB as `r,g,b` |

## test-pattern

Diagnostic: walk LEDs or light them all.

```bash
# Walk one LED at a time (250ms each)
trcc-display test-pattern --mode walk

# Light all LEDs white
trcc-display test-pattern --mode all --color 255,255,255

# Slow walk for photo mapping
trcc-display test-pattern --mode walk --delay_ms 1000
```

Options:
| Flag | Default | Description |
|------|---------|-------------|
| `--mode` | `walk` | `walk` (one at a time) or `all` (every LED) |
| `--color` | `255,255,255` | RGB as `r,g,b` |
| `--delay_ms` | `250` | Per-step delay in walk mode |

## Logging

| Verbosity | Level | Description |
|-----------|-------|-------------|
| (default) | `info` | Startup, profile, connection |
| `-v` | `debug` | Per-frame details, tile values |
| `-vv` | `trace` | Full debug including USB I/O |

Override with `RUST_LOG`:
```bash
RUST_LOG=trcc_display=debug ./target/release/trcc-display run
RUST_LOG=trcc_display=trace,rusb=debug ./target/release/trcc-display run
```