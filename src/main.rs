use anyhow::Result;
use clap::Parser;

use etherwhere::anchors::BUILTIN_ANCHORS;
use etherwhere::probe::{Measurement, ProbeConfig, probe_anchor};
use etherwhere::solver::{constraints_from_measurements, haversine_km, solve};

const DEFAULT_KM_PER_MS: f64 = 102.0;

#[derive(Parser, Debug)]
#[command(
    name = "etherwhere",
    about = "Estimate host location from active network latency measurements"
)]
struct Cli {
    #[arg(long, default_value_t = 3)]
    count: u8,
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
    let config = ProbeConfig {
        ping_count: cli.count,
        timeout_ms: cli.timeout_ms,
        trace_hops: cli.trace_hops,
    };

    let mut measurements = BUILTIN_ANCHORS
        .iter()
        .copied()
        .map(|anchor| probe_anchor(anchor, &config, false))
        .collect::<Vec<_>>();

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

    let constraints = constraints_from_measurements(&measurements, cli.km_per_ms);
    let estimate = solve(&constraints);

    print_report(&measurements, &estimate, cli.km_per_ms);
    Ok(())
}

fn print_report(
    measurements: &[Measurement],
    estimate: &Option<etherwhere::solver::Estimate>,
    km_per_ms: f64,
) {
    println!("Model: RTT upper bound using {km_per_ms:.1} km/ms");
    println!(
        "Limits: anycast, asymmetric routing, and metro-level anchor placement dominate error"
    );
    println!();

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
                    "  {} [{}] {} min={:.2}ms avg={:.2}ms max={:.2}ms mdev={:.2}ms <= {:.0} km",
                    measurement.anchor.label(),
                    measurement.anchor.host,
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
                    let address = hop.address.as_deref().unwrap_or("*");
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
                    format!("{}:{} {}{}", hop.hop, address, rtt, note)
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
