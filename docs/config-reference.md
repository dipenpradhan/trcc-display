# Configuration Reference

All configuration is a single JSON file. Unknown keys are silently ignored, so you can document your config with `$comment` fields.

## File structure

```json
{
    "$comment": "All sections below",
    "usb": { ... },
    "profile": { ... },
    "source": { ... },
    "prometheus": { ... },
    "api": { ... },
    "render": { ... },
    "tiles": [ ... ]
}
```

## USB settings

```json
{
    "usb": {
        "vendor_id": 1046,    // decimal 0x0416
        "product_id": 32769,  // decimal 0x8001
        "interface": 0        // HID interface (usually 0)
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `vendor_id` | `u16` | — | USB vendor ID (decimal) |
| `product_id` | `u16` | — | USB product ID (decimal) |
| `interface` | `u8` | `0` | HID interface number |

## Profile selection

```json
{
    "profile": {
        "dir": "config/profiles",
        "force": null
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `dir` | `string` | — | Directory containing profile JSON files |
| `force` | `string` \| `null` | `null` | Skip PM auto-detect, force profile by name (e.g., `"pa120"`) |

## Metric source

```json
{
    "source": {
        "kind": "prometheus",
        "sensors_command": ["sensors", "-j"]
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `kind` | `string` | `"prometheus"` | Backend: `"prometheus"`, `"prom"`, `"sensors"`, `"lm-sensors"`, `"lmsensors"` |
| `sensors_command` | `[string]` | `["sensors", "-j"]` | Command for lm-sensors source |

## Prometheus settings

```json
{
    "prometheus": {
        "url": "http://localhost:9090",
        "timeout_seconds": 4
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `url` | `string` | `"http://localhost:9090"` | Prometheus base URL |
| `timeout_seconds` | `u64` | `4` | Query timeout in seconds |

## REST API

```json
{
    "api": {
        "enabled": true,
        "bind": "0.0.0.0:9110",
        "preview_enabled": false
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | `bool` | `true` | Set `false` for headless mode |
| `bind` | `string` | `"0.0.0.0:9110"` | Listen address |
| `preview_enabled` | `bool` | `false` | Serve a live LED preview at `/preview` |

When `preview_enabled: true`, the `/preview` routes are added to the API router.
The dashboard is self-contained (embedded HTML/CSS/JS, zero external requests)
and polls `/preview/frame` at 4 Hz. See [API reference](api-reference.md#live-web-preview).

## Render settings

```json
{
    "render": {
        "refresh_seconds": 5,
        "rotate_seconds": 3,
        "tick_ms": 250,
        "temp_unit": "C",
        "indicator_color": [120, 120, 130]
    }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `refresh_seconds` | `u64` | `5` | How often to re-query metrics |
| `rotate_seconds` | `u64` | `3` | Seconds per tile when rotating shared slots |
| `tick_ms` | `u64` | `250` | Frame cadence in milliseconds |
| `temp_unit` | `string` | `"C"` | `"C"` or `"F"` (Celsius tiles auto-convert to F) |
| `indicator_color` | `[u8, u8, u8]` | `[120, 120, 130]` | `[r, g, b]` for always-on / unit LEDs |

## Tiles

```json
{
    "tiles": [
        {
            "name": "gpu_temp",
            "slot": "gpu_temp",
            "query": "max(DCGM_FI_DEV_GPU_TEMP)",
            "unit": "celsius",
            "color": [0, 200, 120],
            "warn": 75,
            "crit": 84,
            "indicators": ["gpu"]
        }
    ]
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `name` | `string` | — | Unique tile name (used in status/errors) |
| `slot` | `string` | — | Profile slot to display in |
| `query` | `string` | — | PromQL expression or `chip/feature` selector |
| `unit` | `string` \| `null` | `null` | Unit marker: `"celsius"`, `"fahrenheit"`, `"percent"` |
| `color` | `[u8, u8, u8]` | `[0, 200, 120]` | Base `[r, g, b]` color |
| `warn` | `f64` \| `null` | `null` | Amber threshold (at or above this value) |
| `crit` | `f64` \| `null` | `null` | Red threshold (at or above this value) |
| `indicators` | `[string]` | `[]` | Indicator groups to light (profile `indicators` keys) |

### Tile query formats

**Prometheus source** — tile `query` is a PromQL instant expression:
```json
{ "query": "max(DCGM_FI_DEV_GPU_TEMP)" }
{ "query": "node_cpu_temp_celsius{cpu='0',core='0'}" }
{ "query": "100 * (1 - avg(rate(node_cpu_seconds_total{mode='idle'}[5m])))" }
```

**lm-sensors source** — tile `query` is a path into the `sensors -j` JSON tree:
```json
{ "query": "k10temp-pci-00c3/Tctl/temp1_input" }
{ "query": "amdgpu-pci-1200/edge" }  // auto-picks first *_input
{ "query": "nct6799-isa-0290/PECI/TSI Agent 0 Calibration" }
```

## Example configs

### Prometheus + REST API

```json
{
    "usb": { "vendor_id": 1046, "product_id": 32769 },
    "profile": { "dir": "config/profiles" },
    "source": { "kind": "prometheus" },
    "prometheus": { "url": "http://prometheus:9090" },
    "api": { "enabled": true },
    "render": { "temp_unit": "C" },
    "tiles": [
        {
            "name": "cpu_temp",
            "slot": "cpu_temp",
            "query": "node_cpu_temp_celsius{cpu='0',core='0'}",
            "unit": "celsius",
            "warn": 75,
            "crit": 85
        },
        {
            "name": "gpu_temp",
            "slot": "gpu_temp",
            "query": "max(DCGM_FI_DEV_GPU_TEMP)",
            "unit": "celsius",
            "warn": 70,
            "crit": 80
        }
    ]
}
```

### lm-sensors (fully local)

```json
{
    "usb": { "vendor_id": 1046, "product_id": 32769 },
    "profile": { "dir": "config/profiles" },
    "source": { "kind": "sensors" },
    "api": { "enabled": false },
    "render": { "temp_unit": "C" },
    "tiles": [
        {
            "name": "cpu",
            "slot": "main",
            "query": "k10temp-pci-00c3/Tctl/temp1_input",
            "unit": "celsius",
            "warn": 75,
            "crit": 85
        }
    ]
}
```