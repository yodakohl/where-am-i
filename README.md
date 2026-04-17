# where-am-i

[![CI](https://github.com/yodakohl/where-am-i/actions/workflows/ci.yml/badge.svg)](https://github.com/yodakohl/where-am-i/actions/workflows/ci.yml)

`where-am-i` ships the `etherwhere` Rust CLI: an experimental network geolocation tool for Ethernet-connected hosts. It estimates physical location from active RTT latency probing, traceroute corridor hints, and a posterior uncertainty model. It is aimed at network geolocation, latency-measurement, and traceroute-assisted multilateration experiments without relying on server-advertised location.

## Install

Build from source:

```bash
cargo build --release
```

Install the CLI directly from GitHub:

```bash
cargo install --git https://github.com/yodakohl/where-am-i etherwhere
```

Linux userland tools required at runtime: `ping`, `tracepath`, `ip`, and `getent`.

## Run

Fast default:

```bash
cargo run --release -- --count 3 --rounds 2
```

Recommended when traceroute hostnames are available:

```bash
cargo run --release -- --count 3 --rounds 2 --trace-hints
```

## Use Cases

- estimate the likely metro or region of an Ethernet-connected Linux host from network measurements
- compare how different access networks change the inferred corridor and uncertainty radius
- inspect whether a result is constrained by local RTT floor, routing corridor, or sparse anchor geometry

## What It Does

- probes a built-in global anchor set with ICMP and TCP fallback
- estimates local access delay and decays cached timing floors over time
- extracts corridor hints from trace hostnames such as regional IX or metro codes
- reports a posterior center, posterior mode, and uncertainty radius

## Example Output

```text
Estimate:
  latitude:  47.8415
  longitude: 13.5302
  posterior mode: 47.5994, 12.9912 (score 511.430)
  confidence radius: 187.2 km
  dominant corridor hub: Vienna (at-vienna-1) (215 km)
```

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

## Related reading

If you use network measurements as one input into a larger alerting or
verification workflow, these two PushMe guides are the closest match:

- [How to verify alerts with primary sources](https://pushme.site/blog/primary-sources-not-recycled-alerts)
  for checking direct evidence before routing an alert onward
- [Real-time alerting checklist for small teams](https://pushme.site/blog/real-time-alerting-checklist)
  for keeping a measurement-driven alert stack usable once it leaves the lab

## Notes

- Linux-only today
- active probing can be noisy on locked-down or monitored networks
- writes a local `.etherwhere-cache` file in the working directory
- best suited for research, experimentation, and operator diagnostics rather than exact end-user geolocation
