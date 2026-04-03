# where-am-i

`where-am-i` is a Rust CLI whose goal is to get the most accurate location estimate possible using only Ethernet-derived signals.

The target is not a single machine, cloud, provider, or city.

The target is:

> any Ethernet-connected device, anywhere in the world

The software should work from generic network evidence, not from assumptions about the current host.

That means:

- no GPS
- no Wi-Fi SSIDs
- no browser geolocation
- no manual coordinates
- no out-of-band user hints

## Scope

This project is meant to be:

- device-agnostic
- provider-agnostic
- country-agnostic
- globally usable

It should get as close as possible to the real physical location of a machine from Ethernet-visible behavior alone.

## Current approach

The current implementation uses active network measurement over the host's Ethernet-connected network path:

1. Probe a set of known anchor endpoints with `ping`
2. Fall back to TCP connect timing when ICMP is filtered but the anchor is still reachable
3. Calibrate and subtract the local TCP handshake bias using anchors that answer both ICMP and TCP
4. Reuse a persistent on-disk lower-envelope cache so repeated runs keep the best physically observed anchor floors
5. Re-probe the fastest and previously missing anchors to build a better lower-envelope RTT set
6. Measure the local egress RTT floor from the default gateway or first routed hop
7. Take the minimum anchor RTT as a propagation-limited upper bound on distance
8. Use the local RTT floor as a soft prior when one anchor is overwhelmingly faster than the rest
9. Optionally collect `tracepath` output for path diagnostics
10. Solve for the latitude/longitude that best fits the measured bounds

This is a physics-inspired approximation, not a claim of exact physical coordinates.

The current code is a first global estimator, not the finished end state.

## What it is trying to do

The project is optimized for one question:

> How close can we get to the machine's real-world location using only what can be learned from Ethernet traffic and Ethernet-reachable hosts?

For any device worldwide, the expected progression is:

- country
- metro
- provider region
- facility/campus, if the network evidence supports it

The final standard is not "works on this host".

The final standard is "works as well as possible on arbitrary Ethernet-connected systems in different networks around the world".

## Current limitations

Accuracy is dominated by network reality, not just the solver:

- routing is asymmetric
- many targets are anycasted or edge-terminated
- anchor coordinates are metro-level, not rack-level
- software RTT timestamps are noisy
- fiber path length is always longer than straight-line distance
- the current bundled anchor set is still too sparse for uniform global precision

Because of that, the tool should be treated as a best-effort estimator.

The current implementation is portable, but not yet equally strong in every region.

## Signals that actually help

Promising signals:

- RTT bounds to dense anchor sets
- TCP handshake RTT when ICMP is blocked
- cross-protocol TCP-vs-ICMP timing bias calibration
- traceroute and tracepath hop structure
- local gateway and upstream topology fingerprints
- repeated lower-envelope measurements over time
- persistent historical floor caching across runs
- hardware timestamping or PTP-grade timing, if available on the device

Low-value or non-viable signals:

- gravitational waves
- moon phases
- solar storms
- generic astronomy data with no device-local sensor

Those are either far too weak, not locally observable on normal Ethernet hardware, or not specific enough to determine a machine's location.

## Usage

Build and run:

```bash
cargo run -- --count 3
```

Enable trace diagnostics for the fastest anchors:

```bash
cargo run -- --count 3 --trace --trace-fastest 3 --trace-hops 6
```

Useful flags:

- `--count`: ICMP samples per anchor
- `--timeout-ms`: timeout per ping batch
- `--trace`: enable `tracepath`
- `--trace-fastest`: only trace the lowest-latency anchors
- `--trace-hops`: maximum trace depth
- `--km-per-ms`: propagation model constant used to convert RTT into a distance cap

Runtime artifacts:

- `.etherwhere-cache`: persistent lower-envelope cache used to improve repeated runs

## Output

The CLI reports:

- estimated latitude and longitude
- nearest bundled metro anchor
- solver stability radius across subset solves
- tightest distance bound
- per-anchor RTT samples
- optional trace hops
- how well the estimate fits each anchor constraint

## Project status

Current implementation:

- built-in global/regional anchor set
- `ping` parser
- `tracepath` parser
- TCP connect timing fallback for ICMP-dark anchors
- TCP handshake bias correction for fallback samples
- persistent lower-envelope cache across runs
- local RTT-floor calibration from gateway or first-hop probes
- second-pass lower-envelope probing for critical anchors
- RTT-to-distance upper bounds
- soft multilateration solver with dominant-anchor prior
- stability radius reporting from subset re-solves

Planned iterations:

- denser regional anchor sets on every continent
- configurable external anchor catalogs
- better path inflation modeling
- anchor calibration from repeated measurements
- tighter confidence radius reporting
- additional Ethernet-only evidence sources when they improve accuracy
- removal of anything that only works because of a specific provider or environment
