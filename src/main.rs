use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use etherwhere::anchors::BUILTIN_ANCHORS;
use etherwhere::cache::{ProbeCache, cached_ping_to_stats};
use etherwhere::probe::{
    LocalRttFloor, Measurement, ProbeConfig, ProbeMethod, measure_local_rtt_floor,
    measure_tcp_handshake, probe_anchor,
};
use etherwhere::solver::{constraints_from_measurements, haversine_km, solve};

const CACHE_PATH: &str = ".etherwhere-cache";
const DEFAULT_KM_PER_MS: f64 = 102.0;

#[derive(Parser, Debug)]
#[command(
    name = "etherwhere",
    about = "Estimate host location from active network latency measurements"
)]
struct Cli {
    #[arg(long, default_value_t = 3)]
    count: u8,
    #[arg(long, default_value_t = 2)]
    rounds: u8,
    #[arg(long, default_value_t = 1000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 8)]
    trace_hops: u8,
    #[arg(long, default_value_t = 3)]
    trace_fastest: usize,
    #[arg(long, default_value_t = false)]
    trace: bool,
    #[arg(long, default_value_t = DEFAULT_KM_PER_MS)]
    km_per_ms: f64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cache_path = Path::new(CACHE_PATH);
    let mut cache = ProbeCache::load(cache_path).unwrap_or_default();
    let config = ProbeConfig {
        ping_count: cli.count,
        rounds: cli.rounds,
        timeout_ms: cli.timeout_ms,
        trace_hops: cli.trace_hops,
    };

    let mut measurements = BUILTIN_ANCHORS
        .iter()
        .copied()
        .map(|anchor| probe_anchor(anchor, &config, false))
        .collect::<Vec<_>>();
    refine_fastest_measurements(&mut measurements, &config);
    let tcp_bias_ms = merge_tcp_bias_ms(calibrate_tcp_bias_ms(&measurements, &config), &cache);
    apply_tcp_bias_correction(&mut measurements, tcp_bias_ms);
    apply_cached_anchor_floors(&mut measurements, &cache);

    if cli.trace {
        let mut ranked = measurements
            .iter()
            .enumerate()
            .filter_map(|(index, measurement)| {
                measurement.ping.as_ref().map(|ping| (index, ping.min_ms))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| left.1.total_cmp(&right.1));

        for (index, _) in ranked.into_iter().take(cli.trace_fastest) {
            measurements[index] = probe_anchor(measurements[index].anchor, &config, true);
        }
    }

    let local_rtt_floor = merge_local_rtt_floor(measure_local_rtt_floor(&config), &cache);
    let shared_rtt_floor_ms =
        calibrated_shared_rtt_floor_ms(&measurements, local_rtt_floor.as_ref());
    let constraints = constraints_from_measurements(&measurements, cli.km_per_ms);
    let hints = Vec::new();
    let estimate = solve(&constraints, &hints, shared_rtt_floor_ms);

    print_report(
        &measurements,
        &estimate,
        cli.km_per_ms,
        &local_rtt_floor,
        shared_rtt_floor_ms,
        tcp_bias_ms,
        cache.anchor_floors.len(),
    );

    cache.update_from_measurements(&measurements);
    cache.update_local_rtt_floor(local_rtt_floor.as_ref());
    cache.update_tcp_bias(tcp_bias_ms);
    let _ = cache.save(cache_path);

    Ok(())
}

fn refine_fastest_measurements(measurements: &mut [Measurement], config: &ProbeConfig) {
    let mut ranked = measurements
        .iter()
        .enumerate()
        .filter_map(|(index, measurement)| {
            measurement.ping.as_ref().map(|ping| (index, ping.min_ms))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.1.total_cmp(&right.1));

    let Some((_, fastest_rtt_ms)) = ranked.first().copied() else {
        return;
    };

    let threshold_ms = fastest_rtt_ms.mul_add(4.0, 0.0).max(25.0);
    let mut refine_indices = ranked
        .into_iter()
        .filter_map(|(index, rtt_ms)| (rtt_ms <= threshold_ms).then_some(index))
        .take(6)
        .collect::<BTreeSet<_>>();

    for (index, measurement) in measurements.iter().enumerate() {
        if measurement.ping.is_none() {
            refine_indices.insert(index);
        }
    }

    let refine_config = ProbeConfig {
        rounds: config.rounds.max(3) + 2,
        ping_count: config.ping_count.max(1),
        ..*config
    };

    for index in refine_indices {
        let refined = probe_anchor(measurements[index].anchor, &refine_config, false);
        if should_replace_measurement(&measurements[index], &refined) {
            measurements[index] = refined;
        }
    }
}

fn should_replace_measurement(current: &Measurement, refined: &Measurement) -> bool {
    match (&current.ping, &refined.ping) {
        (Some(current), Some(next)) => next.min_ms < current.min_ms,
        (None, Some(_)) => true,
        _ => false,
    }
}

fn calibrated_shared_rtt_floor_ms(
    measurements: &[Measurement],
    local_rtt_floor: Option<&LocalRttFloor>,
) -> f64 {
    let Some(local_rtt_floor) = local_rtt_floor else {
        return 0.0;
    };

    let Some(fastest_anchor_rtt_ms) = measurements
        .iter()
        .filter_map(|measurement| measurement.ping.as_ref().map(|ping| ping.min_ms))
        .min_by(|left, right| left.total_cmp(right))
    else {
        return 0.0;
    };

    local_rtt_floor.min_ms.min(fastest_anchor_rtt_ms * 0.75)
}

fn merge_tcp_bias_ms(current_tcp_bias_ms: f64, cache: &ProbeCache) -> f64 {
    match cache.tcp_bias_ms {
        Some(cached_tcp_bias_ms) if current_tcp_bias_ms > 0.0 => {
            current_tcp_bias_ms.min(cached_tcp_bias_ms)
        }
        Some(cached_tcp_bias_ms) => cached_tcp_bias_ms,
        None => current_tcp_bias_ms,
    }
}

fn merge_local_rtt_floor(
    current_local_rtt_floor: Option<LocalRttFloor>,
    cache: &ProbeCache,
) -> Option<LocalRttFloor> {
    match (current_local_rtt_floor, cache.local_rtt_floor_ms) {
        (Some(current), Some(cached_min_ms)) if cached_min_ms < current.min_ms => {
            Some(LocalRttFloor {
                target: CACHE_PATH.to_string(),
                source: "historical lower envelope".to_string(),
                min_ms: cached_min_ms,
            })
        }
        (Some(current), _) => Some(current),
        (None, Some(cached_min_ms)) => Some(LocalRttFloor {
            target: CACHE_PATH.to_string(),
            source: "historical lower envelope".to_string(),
            min_ms: cached_min_ms,
        }),
        (None, None) => None,
    }
}

fn apply_cached_anchor_floors(measurements: &mut [Measurement], cache: &ProbeCache) {
    for measurement in measurements {
        let Some(cached_ping) = cache.anchor_floors.get(measurement.anchor.id) else {
            continue;
        };

        let should_use_cache = match &measurement.ping {
            Some(current_ping) => cached_ping.min_ms < current_ping.min_ms,
            None => true,
        };

        if should_use_cache {
            measurement.ping = Some(cached_ping_to_stats(cached_ping));
            measurement.notes.push(format!(
                "used cached lower envelope {:.2} ms via {}",
                cached_ping.min_ms,
                cached_ping.method.label()
            ));
        }
    }
}

fn calibrate_tcp_bias_ms(measurements: &[Measurement], config: &ProbeConfig) -> f64 {
    let mut ranked = measurements
        .iter()
        .filter_map(|measurement| {
            let ping = measurement.ping.as_ref()?;
            (ping.method == ProbeMethod::Icmp).then_some((measurement, ping.min_ms))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.1.total_cmp(&right.1));

    let mut deltas = Vec::new();
    for (measurement, icmp_min_ms) in ranked.into_iter().take(4) {
        let target = measurement
            .resolved_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| measurement.anchor.host.to_string());
        let Some(tcp_stats) = measure_tcp_handshake(&target, config.timeout_ms) else {
            continue;
        };
        let delta = tcp_stats.min_ms - icmp_min_ms;
        if delta.is_finite() && delta > 0.0 && delta < 10.0 {
            deltas.push(delta);
        }
    }

    if deltas.is_empty() {
        return 0.0;
    }

    deltas.sort_by(|left, right| left.total_cmp(right));
    deltas[deltas.len() / 2]
}

fn apply_tcp_bias_correction(measurements: &mut [Measurement], tcp_bias_ms: f64) {
    if tcp_bias_ms <= 0.0 {
        return;
    }

    for measurement in measurements {
        let Some(ping) = &mut measurement.ping else {
            continue;
        };
        if !matches!(ping.method, ProbeMethod::Tcp { .. }) {
            continue;
        }

        ping.min_ms = (ping.min_ms - tcp_bias_ms).max(0.05);
        ping.avg_ms = (ping.avg_ms - tcp_bias_ms).max(ping.min_ms);
        ping.max_ms = (ping.max_ms - tcp_bias_ms).max(ping.avg_ms);
        measurement.notes.push(format!(
            "applied TCP bias correction of {tcp_bias_ms:.2} ms"
        ));
    }
}

fn print_report(
    measurements: &[Measurement],
    estimate: &Option<etherwhere::solver::Estimate>,
    km_per_ms: f64,
    local_rtt_floor: &Option<LocalRttFloor>,
    shared_rtt_floor_ms: f64,
    tcp_bias_ms: f64,
    cached_anchor_count: usize,
) {
    println!("Model: RTT upper bound using {km_per_ms:.1} km/ms");
    println!(
        "Limits: anycast, asymmetric routing, and metro-level anchor placement dominate error"
    );
    println!();

    if let Some(local_rtt_floor) = local_rtt_floor {
        println!("Calibration:");
        println!(
            "  local RTT floor: {:.2} ms via {} ({})",
            local_rtt_floor.min_ms, local_rtt_floor.target, local_rtt_floor.source
        );
        println!(
            "  shared floor used in bounds: {:.2} ms",
            shared_rtt_floor_ms
        );
        if tcp_bias_ms > 0.0 {
            println!("  TCP handshake bias correction: {:.2} ms", tcp_bias_ms);
        }
        if cached_anchor_count > 0 {
            println!("  historical anchor floors loaded: {cached_anchor_count}");
        }
        println!();
    }

    match estimate {
        Some(estimate) => {
            let nearest_anchor = estimate
                .constraints
                .iter()
                .min_by(|left, right| left.distance_km.total_cmp(&right.distance_km));
            let tightest = estimate
                .constraints
                .iter()
                .min_by(|left, right| left.upper_bound_km.total_cmp(&right.upper_bound_km));
            let worst_overflow = estimate
                .constraints
                .iter()
                .map(|constraint| constraint.overflow_km)
                .fold(0.0_f64, f64::max);

            println!("Estimate:");
            println!("  latitude:  {:.4}", estimate.latitude);
            println!("  longitude: {:.4}", estimate.longitude);
            println!("  anchors used: {}", estimate.constraints.len());
            if let Some(anchor) = nearest_anchor {
                println!(
                    "  nearest bundled metro: {} ({:.0} km)",
                    anchor.anchor.label(),
                    anchor.distance_km
                );
            }
            if let Some(anchor) = tightest {
                println!(
                    "  tightest bound: {} <= {:.0} km",
                    anchor.anchor.label(),
                    anchor.upper_bound_km
                );
            }
            println!("  solver score: {:.3}", estimate.score);
            println!(
                "  fitted base overhead: {:.2} ms",
                estimate.fitted_overhead_ms
            );
            println!(
                "  fitted effective speed: {:.1} km/ms",
                estimate.fitted_km_per_ms
            );
            println!(
                "  weighted RTT residual: {:.2} ms",
                estimate.weighted_rmse_ms
            );
            println!(
                "  stability radius: {:.1} km",
                estimate.confidence_radius_km
            );
            println!("  max constraint overflow: {:.1} km", worst_overflow);
            if estimate.constraints.len() < 3 {
                println!("  warning: geometry is underdetermined with fewer than 3 anchors");
            }
            println!();
        }
        None => {
            println!("Estimate: unavailable");
            println!("Reason: no anchors returned usable RTT samples");
            println!();
        }
    }

    println!("Measurements:");
    for measurement in measurements {
        match &measurement.ping {
            Some(ping) => {
                let upper_bound_km = ping.upper_bound_km(km_per_ms);
                println!(
                    "  {} [{}] {} {} min={:.2}ms avg={:.2}ms max={:.2}ms mdev={:.2}ms <= {:.0} km",
                    measurement.anchor.label(),
                    measurement.anchor.host,
                    ping.method.label(),
                    measurement
                        .resolved_ip
                        .map(|ip| ip.to_string())
                        .unwrap_or_else(|| "unresolved".to_string()),
                    ping.min_ms,
                    ping.avg_ms,
                    ping.max_ms,
                    ping.mdev_ms,
                    upper_bound_km
                );
            }
            None => {
                println!(
                    "  {} [{}] no usable ping sample",
                    measurement.anchor.label(),
                    measurement.anchor.host
                );
            }
        }

        if let Some(trace) = &measurement.trace {
            let rendered = trace
                .hops
                .iter()
                .map(|hop| {
                    let address = hop
                        .hostname
                        .as_deref()
                        .or(hop.address.as_deref())
                        .unwrap_or("*");
                    let rtt = hop
                        .rtt_ms
                        .map(|value| format!("{value:.2}ms"))
                        .unwrap_or_else(|| "?".to_string());
                    let note = hop
                        .note
                        .as_deref()
                        .filter(|note| *note != "no reply")
                        .map(|note| format!(" {note}"))
                        .unwrap_or_default();
                    let ip = hop
                        .hostname
                        .as_ref()
                        .zip(hop.address.as_ref())
                        .map(|(_, address)| format!(" [{address}]"))
                        .unwrap_or_default();
                    format!("{}:{}{} {}{}", hop.hop, address, ip, rtt, note)
                })
                .collect::<Vec<_>>()
                .join(" | ");
            println!("    trace: {rendered}");
        }

        for note in &measurement.notes {
            println!("    note: {note}");
        }
    }

    if let Some(estimate) = estimate {
        println!();
        println!("Anchor fit:");
        let mut constraints = estimate.constraints.clone();
        constraints.sort_by(|left, right| left.upper_bound_km.total_cmp(&right.upper_bound_km));
        for constraint in constraints {
            let delta = constraint.upper_bound_km - constraint.distance_km;
            let fit = if delta >= 0.0 {
                format!("inside by {:.0} km", delta)
            } else {
                format!("outside by {:.0} km", delta.abs())
            };
            println!(
                "  {} rtt={:.2}ms bound={:.0} km distance={:.0} km {}",
                constraint.anchor.label(),
                constraint.min_rtt_ms,
                constraint.upper_bound_km,
                constraint.distance_km,
                fit
            );
        }

        if let Some(closest) = BUILTIN_ANCHORS.iter().min_by(|left, right| {
            haversine_km(
                estimate.latitude,
                estimate.longitude,
                left.latitude,
                left.longitude,
            )
            .total_cmp(&haversine_km(
                estimate.latitude,
                estimate.longitude,
                right.latitude,
                right.longitude,
            ))
        }) {
            let error = haversine_km(
                estimate.latitude,
                estimate.longitude,
                closest.latitude,
                closest.longitude,
            );
            println!();
            println!(
                "Closest bundled city-center: {} at {:.0} km",
                closest.label(),
                error
            );
        }
    }
}
