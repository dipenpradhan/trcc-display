# Architecture

## Overview

```
metric source ──▶ engine ──▶ render ──▶ protocol ──▶ USB worker ──▶ cooler
(prometheus |     (loop +    (7-seg     (packet      (libusb, own
 sensors -j)      overrides)  digits)    framing)     thread)
      ▲
 REST API (optional) sets overrides / raw frames
```

## Module layout

```
trcc_display/
├── lib.rs          — crate root, public module re-exports
├── main.rs         — CLI entry point, subcommand dispatch
├── protocol.rs     — pure wire framing + 7-segment font (no I/O)
├── profile.rs      — per-model LED geometry, loaded from JSON
├── render.rs       — value → physical-order color frame
├── config.rs       — JSON config file parsing
├── state.rs        — shared runtime state (Shared, Override, RawOverride)
├── util.rs         — shared utilities (lock helpers)
├── source.rs       — unified metric source enum
├── prometheus.rs   — Prometheus HTTP client
├── sensors.rs      — lm-sensors JSON tree selection
├── engine.rs       — render loop + frame building
├── api.rs          — REST API (axum)
└── usb.rs          — libusb worker (handshake, probe cache, frame writes)
```

## Data flow

### 1. Startup

```
main.rs
  ├─ Load config (config.rs)
  ├─ Load profiles (profile.rs)
  ├─ Spawn USB worker thread (usb.rs)
  ├─ Start engine loop (engine.rs)
  └─ Start REST API (api.rs, optional)
```

### 2. USB initialization

```
USB worker thread:
  ├─ Open device (rusb)
  ├─ Send init packet (protocol::init_packet)
  ├─ Read handshake response
  ├─ Parse PM/SUB bytes (protocol::parse_handshake)
  ├─ Cache result (usb::cache_save)
  └─ Update Shared.pm/sub
```

### 3. Render loop (engine)

```
Every tick_ms (default 250ms):
  ├─ Resolve profile (by PM byte or force)
  ├─ Refresh metrics (every refresh_seconds)
  │   ├─ Source.fetch(tiles)
  │   └─ Update Shared.values/errors
  ├─ Expire stale overrides
  ├─ Build frame
  │   ├─ Raw override → immediate return
  │   ├─ Group tiles by slot
  │   ├─ Rotate tiles sharing a slot
  │   ├─ Apply slot overrides
  │   ├─ render::frame(profile, slots, indicator_color)
  │   └─ protocol::data_packet(frame)
  └─ Send to USB worker via SyncSender
```

### 4. USB worker

```
Loop:
  ├─ Connect (open + handshake)
  ├─ write_loop:
  │   ├─ Wait for packet from channel
  │   ├─ protocol::chunks(packet)
  │   └─ write_interrupt for each chunk
  └─ On error: reconnect after 3s
```

## Key abstractions

### Source enum

```rust
pub enum Source {
    Prometheus(PromClient),
    Sensors(SensorsSource),
}
```

Both backends converge on one `fetch` method. The engine never knows which backend is active.

### Profile as data

```json
{
    "name": "COOLER",
    "mask_size": 30,
    "slots": { "main": { "digits": [...] } }
}
```

Add a new cooler by dropping a JSON file. No Rust changes needed.

### Shared state

```rust
pub struct Shared {
    pub connected: bool,
    pub values: HashMap<String, f64>,
    pub overrides: HashMap<String, Override>,
    // ...
}
```

Shared via `Arc<Mutex<Shared>>` between engine, USB worker, and API. All fields are plain data.

## Threading model

```
Main thread (async):
  └─ Engine loop (tokio task)
       └─ REST API (tokio task, optional)

USB thread (sync):
  └─ libusb worker
```

The engine and USB worker communicate via a bounded `SyncSender<Vec<u8>>` (capacity 2). If the USB worker is busy, frames are dropped (the next tick sends a fresh one).

## Design principles

1. **No unsafe code** — all code is safe Rust
2. **Pure leaf modules** — `protocol`, `sensors`, `prometheus` have zero internal deps
3. **Data-driven** — config and profiles are JSON
4. **Async/sync separation** — tokio for I/O, dedicated thread for libusb
5. **Per-tile errors** — one bad query doesn't blank the display