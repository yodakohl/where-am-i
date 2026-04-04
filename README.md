# where-am-i

[![CI](https://github.com/yodakohl/where-am-i/actions/workflows/ci.yml/badge.svg)](https://github.com/yodakohl/where-am-i/actions/workflows/ci.yml)

`where-am-i` ships the `etherwhere` Rust CLI: an experimental network geolocation tool for Ethernet-connected hosts. It estimates location from active RTT probing, traceroute corridor hints, and a posterior uncertainty model. It does not rely on server-advertised location.

## Build

Linux userland tools required: `ping`, `tracepath`, `ip`, and `getent`.

```bash
cargo build --release
```

## Run

Fast default:

```bash
cargo run --release -- --count 3 --rounds 2
```

Recommended when traceroute hostnames are available:

```bash
cargo run --release -- --count 3 --rounds 2 --trace-hints
```

## What It Does

- probes a built-in global anchor set with ICMP and TCP fallback
- estimates local access delay and decays cached timing floors over time
- extracts corridor hints from trace hostnames such as regional IX or metro codes
- reports a posterior center, posterior mode, and uncertainty radius

## Read The Output

- `latitude` / `longitude`: posterior center
- `posterior mode`: best local fit when it differs materially from the center
- `confidence radius`: the main uncertainty number to watch
- `dominant corridor hub`: likely transit hub, not the machine's exact city
- `nearest bundled metro`: nearest anchor in the built-in set, not necessarily the estimate itself

## Expectations

This is not GPS. It is an active network measurement tool, so accuracy depends heavily on access-network overhead, anchor geometry, routing asymmetry, and whether traceroute reveals useful corridor information.

Reasonable expectations:

- favorable metro-connected hosts with nearby anchors and clean routes can land within a few to a few dozen kilometers
- noisier residential or high-overhead links can drift to regional-scale error, often tens to hundreds of kilometers
- a large `confidence radius` means the estimate should be treated as a broad region, not a precise point

Runs with `--trace` or `--trace-hints` are usually slower but can improve corridor inference.

## Notes

- Linux-only today
- active probing can be noisy on locked-down or monitored networks
- writes a local `.etherwhere-cache` file in the working directory
