# where-am-i

`where-am-i` ships the `etherwhere` Rust CLI: a physics-first network geolocation tool for Ethernet-connected hosts. It uses active RTT probing, traceroute corridor hints, and posterior uncertainty. It does not trust server-advertised location.

Keywords: Rust, CLI, geolocation, network latency, RTT, traceroute, Ethernet, multilateration.

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
- reports a posterior center, posterior mode, and confidence radius

## Read The Output

- `latitude` / `longitude`: posterior center
- `posterior mode`: best local fit when it differs materially from the center
- `confidence radius`: the number to watch for uncertainty
- `dominant corridor hub`: likely transit hub, not the machine's exact city
- `nearest bundled metro`: nearest anchor in the built-in set, not necessarily the estimate itself

## Ballpark Results

This is metro-to-regional geolocation, not GPS.

Recent measured and regression-backed results:

- low-overhead Frankfurt-style metro case: often single-digit kilometers, roughly `0-5 km` in repeated local measurements
- Salzburg / Vienna-corridor live run: about `24-36 km` from Salzburg, with `92.8 km` posterior radius and `187.2 km` confidence radius
- harder high-overhead Austria replay: regression target is under `170 km` error while keeping uncertainty broad instead of faking precision

Expect results to worsen on noisy residential links, asymmetric routing, anycast-heavy paths, or sparse anchor geometry. Runs with `--trace` or `--trace-hints` are usually slower but can improve corridor inference.

## Notes

- Linux-only today
- active probing can be noisy on locked-down or monitored networks
- writes a local `.etherwhere-cache` file in the working directory
