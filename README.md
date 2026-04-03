# where-am-i

`where-am-i` is a Rust CLI that estimates the physical location of an Ethernet-connected machine from network timing only.

It probes a built-in set of internet anchors, measures RTT with ICMP and TCP fallback, calibrates local network delay, and fits the best latitude/longitude estimate it can from those constraints.

## Build

```bash
cargo build --release
```

## Run

```bash
cargo run --release -- --count 3 --rounds 2
```

Optional trace output:

```bash
cargo run --release -- --count 3 --rounds 2 --trace --trace-fastest 3
```

## Output

The CLI reports:

- estimated latitude and longitude
- nearest bundled metro anchor
- stability radius
- per-anchor timing measurements

It also writes a local `.etherwhere-cache` file to reuse the best observed timing floors across runs.
