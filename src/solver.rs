use crate::anchors::Anchor;
use crate::hints::LocationHint;
use crate::probe::Measurement;

const EARTH_RADIUS_KM: f64 = 6371.0;
const MIN_KM_PER_MS: f64 = 102.0;
const MIN_MS_PER_KM: f64 = 1.0 / MIN_KM_PER_MS;

#[derive(Debug, Clone, Copy)]
pub struct Constraint {
    pub anchor: Anchor,
    pub upper_bound_km: f64,
    pub min_rtt_ms: f64,
    pub jitter_ms: f64,
    pub quality_weight: f64,
}

#[derive(Debug, Clone)]
pub struct EvaluatedConstraint {
    pub anchor: Anchor,
    pub min_rtt_ms: f64,
    pub upper_bound_km: f64,
    pub distance_km: f64,
    pub overflow_km: f64,
}

#[derive(Debug, Clone)]
pub struct EvaluatedHint {
    pub anchor: Anchor,
    pub weight: f64,
    pub source: String,
    pub distance_km: f64,
}

#[derive(Debug, Clone)]
pub struct Estimate {
    pub latitude: f64,
    pub longitude: f64,
    pub score: f64,
    pub mode_latitude: f64,
    pub mode_longitude: f64,
    pub mode_score: f64,
    pub fitted_overhead_ms: f64,
    pub fitted_km_per_ms: f64,
    pub weighted_rmse_ms: f64,
    pub confidence_radius_km: f64,
    pub posterior_radius_km: f64,
    pub stability_radius_km: f64,
    pub constraints: Vec<EvaluatedConstraint>,
    pub hints: Vec<EvaluatedHint>,
    pub dominant_corridor_hint: Option<EvaluatedHint>,
}

pub fn constraints_from_measurements(
    measurements: &[Measurement],
    km_per_ms: f64,
) -> Vec<Constraint> {
    measurements
        .iter()
        .filter_map(|measurement| {
            measurement.ping.as_ref().map(|ping| Constraint {
                anchor: measurement.anchor,
                upper_bound_km: ping.upper_bound_km(km_per_ms),
                min_rtt_ms: ping.min_ms,
                jitter_ms: effective_jitter_ms(measurement, ping),
                quality_weight: measurement_quality_weight(measurement, ping),
            })
        })
        .collect()
}

fn effective_jitter_ms(measurement: &Measurement, ping: &crate::probe::PingStats) -> f64 {
    let spread_ms = (ping.avg_ms - ping.min_ms).max(0.0);
    ping.mdev_ms.max(spread_ms * 0.75) + measurement.cache_uncertainty_ms
}

fn measurement_quality_weight(measurement: &Measurement, ping: &crate::probe::PingStats) -> f64 {
    let transport_penalty = if matches!(ping.method, crate::probe::ProbeMethod::Tcp { .. }) {
        0.25
    } else {
        0.0
    };
    let jitter_penalty = effective_jitter_ms(measurement, ping) / 3.0;
    let quality = 1.0 / (1.0 + transport_penalty + jitter_penalty);
    quality.clamp(0.12, 1.0)
}

pub fn solve(
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> Option<Estimate> {
    let mode = solve_once(constraints, hints, shared_rtt_floor_ms)?;
    let stability_radius_km = stability_radius_km(mode, constraints, hints, shared_rtt_floor_ms);
    let posterior = posterior_summary(mode, constraints, hints, shared_rtt_floor_ms);
    let center = candidate(
        posterior.latitude,
        posterior.longitude,
        constraints,
        hints,
        shared_rtt_floor_ms,
    );
    let confidence_radius_km = stability_radius_km.max(posterior.radius_km);
    let dominant_corridor_hint =
        dominant_corridor_hint(posterior.latitude, posterior.longitude, constraints, hints);

    Some(build_estimate(
        center,
        mode,
        constraints,
        hints,
        confidence_radius_km,
        posterior.radius_km,
        stability_radius_km,
        dominant_corridor_hint,
    ))
}

fn solve_once(
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> Option<Candidate> {
    if constraints.is_empty() {
        return None;
    }

    let mut best = initial_candidates(constraints)
        .into_iter()
        .map(|(lat, lon)| candidate(lat, lon, constraints, hints, shared_rtt_floor_ms))
        .min_by(|left, right| left.score.total_cmp(&right.score))?;

    for step in [8.0, 2.0, 0.5, 0.1, 0.02, 0.005, 0.001] {
        best = refine(
            best.latitude,
            best.longitude,
            step,
            constraints,
            hints,
            shared_rtt_floor_ms,
            best.score,
        );
    }

    Some(best)
}

fn build_estimate(
    best: Candidate,
    mode: Candidate,
    constraints: &[Constraint],
    hints: &[LocationHint],
    confidence_radius_km: f64,
    posterior_radius_km: f64,
    stability_radius_km: f64,
    dominant_corridor_hint: Option<EvaluatedHint>,
) -> Estimate {
    Estimate {
        latitude: best.latitude,
        longitude: best.longitude,
        score: best.score,
        mode_latitude: mode.latitude,
        mode_longitude: mode.longitude,
        mode_score: mode.score,
        fitted_overhead_ms: best.fit.overhead_ms,
        fitted_km_per_ms: 1.0 / best.fit.ms_per_km,
        weighted_rmse_ms: best.fit.weighted_rmse_ms,
        confidence_radius_km,
        posterior_radius_km,
        stability_radius_km,
        constraints: constraints
            .iter()
            .map(|constraint| {
                let distance_km = haversine_km(
                    best.latitude,
                    best.longitude,
                    constraint.anchor.latitude,
                    constraint.anchor.longitude,
                );
                EvaluatedConstraint {
                    anchor: constraint.anchor,
                    min_rtt_ms: constraint.min_rtt_ms,
                    upper_bound_km: constraint.upper_bound_km,
                    distance_km,
                    overflow_km: (distance_km - constraint.upper_bound_km).max(0.0),
                }
            })
            .collect(),
        hints: hints
            .iter()
            .map(|hint| EvaluatedHint {
                anchor: hint.anchor,
                weight: hint.weight,
                source: hint.source.clone(),
                distance_km: haversine_km(
                    best.latitude,
                    best.longitude,
                    hint.anchor.latitude,
                    hint.anchor.longitude,
                ),
            })
            .collect(),
        dominant_corridor_hint,
    }
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    latitude: f64,
    longitude: f64,
    score: f64,
    fit: LatencyFit,
}

#[derive(Debug, Clone, Copy)]
struct LatencyFit {
    overhead_ms: f64,
    ms_per_km: f64,
    weighted_rmse_ms: f64,
}

fn initial_candidates(constraints: &[Constraint]) -> Vec<(f64, f64)> {
    let mut candidates = Vec::new();

    for lat in (-60..=75).step_by(15) {
        for lon in (-180..=180).step_by(15) {
            candidates.push((lat as f64, lon as f64));
        }
    }

    for constraint in constraints {
        candidates.push((constraint.anchor.latitude, constraint.anchor.longitude));
    }

    candidates.push(weighted_centroid(constraints));
    candidates
}

fn weighted_centroid(constraints: &[Constraint]) -> (f64, f64) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut total = 0.0;

    for constraint in constraints {
        let weight = 1.0 / constraint.upper_bound_km.max(25.0).powi(2);
        let lat = constraint.anchor.latitude.to_radians();
        let lon = constraint.anchor.longitude.to_radians();
        x += weight * lat.cos() * lon.cos();
        y += weight * lat.cos() * lon.sin();
        z += weight * lat.sin();
        total += weight;
    }

    if total == 0.0 {
        return (0.0, 0.0);
    }

    x /= total;
    y /= total;
    z /= total;

    let lon = y.atan2(x);
    let hyp = (x.powi(2) + y.powi(2)).sqrt();
    let lat = z.atan2(hyp);
    (lat.to_degrees(), lon.to_degrees())
}

fn refine(
    latitude: f64,
    longitude: f64,
    step: f64,
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
    current_score: f64,
) -> Candidate {
    let fit = fit_latency_model(latitude, longitude, constraints);
    let mut best = Candidate {
        latitude,
        longitude,
        score: current_score,
        fit,
    };

    for lat_step in -8..=8 {
        for lon_step in -8..=8 {
            let next_lat = clamp_latitude(latitude + f64::from(lat_step) * step);
            let next_lon = normalize_longitude(longitude + f64::from(lon_step) * step);
            let candidate = candidate(next_lat, next_lon, constraints, hints, shared_rtt_floor_ms);
            if candidate.score < best.score {
                best = candidate;
            }
        }
    }

    best
}

fn candidate(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> Candidate {
    let fit = fit_latency_model(latitude, longitude, constraints);
    Candidate {
        latitude,
        longitude,
        score: objective(
            latitude,
            longitude,
            constraints,
            hints,
            fit,
            shared_rtt_floor_ms,
        ),
        fit,
    }
}

fn objective(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    hints: &[LocationHint],
    fit: LatencyFit,
    shared_rtt_floor_ms: f64,
) -> f64 {
    let upper_bound_score: f64 = constraints
        .iter()
        .map(|constraint| {
            let distance_km = haversine_km(
                latitude,
                longitude,
                constraint.anchor.latitude,
                constraint.anchor.longitude,
            );
            let overflow_km = (distance_km - constraint.upper_bound_km).max(0.0);
            let scale = constraint.upper_bound_km.max(100.0);
            1000.0 * constraint.quality_weight * (overflow_km / scale).powi(2)
        })
        .sum();

    let hint_score: f64 = hints
        .iter()
        .map(|hint| {
            if !hint.supporting_anchors.is_empty() {
                return 0.0;
            }

            let distance_km = haversine_km(
                latitude,
                longitude,
                hint.anchor.latitude,
                hint.anchor.longitude,
            );
            let soft_radius_km = 450.0;
            let local_bias = hint.weight * distance_km / 400.0;
            let overflow_km = (distance_km - soft_radius_km).max(0.0);

            local_bias + 25.0 * hint.weight * (overflow_km / soft_radius_km.max(150.0)).powi(2)
        })
        .sum();
    let corridor_score = trace_corridor_score(latitude, longitude, constraints, hints);

    let dominant_anchor_score =
        dominant_local_anchor_score(latitude, longitude, constraints, shared_rtt_floor_ms);
    let relative_spread_score = relative_spread_score(latitude, longitude, constraints);
    let pairwise_order_score = pairwise_order_score(latitude, longitude, constraints);
    let regional_cluster_score = regional_cluster_score(latitude, longitude, constraints);

    let fit_penalty = 80.0 * fit.weighted_rmse_ms
        + 0.5 * fit.overhead_ms
        + 2000.0 * (fit.ms_per_km - MIN_MS_PER_KM).max(0.0);

    upper_bound_score
        + fit_penalty
        + hint_score
        + corridor_score
        + dominant_anchor_score
        + relative_spread_score
        + pairwise_order_score
        + regional_cluster_score
}

#[derive(Debug, Clone, Copy)]
struct PosteriorSummary {
    latitude: f64,
    longitude: f64,
    radius_km: f64,
}

fn trace_corridor_score(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    hints: &[LocationHint],
) -> f64 {
    hints
        .iter()
        .filter(|hint| !hint.supporting_anchors.is_empty())
        .map(|hint| trace_corridor_score_for_hint(latitude, longitude, constraints, hint))
        .sum()
}

fn trace_corridor_score_for_hint(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    hint: &LocationHint,
) -> f64 {
    let supporting_constraints = hint
        .supporting_anchors
        .iter()
        .filter_map(|support_anchor| {
            constraints
                .iter()
                .find(|constraint| constraint.anchor.id == support_anchor.id)
        })
        .collect::<Vec<_>>();
    if supporting_constraints.is_empty() {
        return 0.0;
    }

    let corridor_base = |constraint: &Constraint| {
        corridor_path_km(
            latitude,
            longitude,
            hint.anchor.latitude,
            hint.anchor.longitude,
            constraint.anchor.latitude,
            constraint.anchor.longitude,
        ) * MIN_MS_PER_KM
    };

    let shared_overhead_ms = supporting_constraints
        .iter()
        .map(|constraint| constraint.min_rtt_ms - corridor_base(constraint))
        .fold(f64::INFINITY, f64::min)
        .max(0.0);

    supporting_constraints
        .into_iter()
        .map(|constraint| {
            let predicted_min_ms = shared_overhead_ms + corridor_base(constraint);
            let residual_ms = constraint.min_rtt_ms - predicted_min_ms;
            let sigma_ms = 1.0 + constraint.jitter_ms.max(0.25);
            let underflow_ms = (-residual_ms).max(0.0);
            let stretch_ms = residual_ms.max(0.0);
            let weight = hint.weight * constraint.quality_weight;

            weight
                * (40.0 * (underflow_ms / sigma_ms).powi(2)
                    + 0.35 * (stretch_ms / (sigma_ms * 2.5)).powi(2))
        })
        .sum()
}

fn corridor_path_km(
    latitude: f64,
    longitude: f64,
    hub_latitude: f64,
    hub_longitude: f64,
    anchor_latitude: f64,
    anchor_longitude: f64,
) -> f64 {
    haversine_km(latitude, longitude, hub_latitude, hub_longitude)
        + haversine_km(
            hub_latitude,
            hub_longitude,
            anchor_latitude,
            anchor_longitude,
        )
}

fn dominant_corridor_hint(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    hints: &[LocationHint],
) -> Option<EvaluatedHint> {
    let hint = hints
        .iter()
        .filter(|hint| !hint.supporting_anchors.is_empty())
        .min_by(|left, right| {
            trace_corridor_score_for_hint(latitude, longitude, constraints, left).total_cmp(
                &trace_corridor_score_for_hint(latitude, longitude, constraints, right),
            )
        })?;

    Some(EvaluatedHint {
        anchor: hint.anchor,
        weight: hint.weight,
        source: hint.source.clone(),
        distance_km: haversine_km(
            latitude,
            longitude,
            hint.anchor.latitude,
            hint.anchor.longitude,
        ),
    })
}

fn posterior_summary(
    mode: Candidate,
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> PosteriorSummary {
    let bootstrap_modes = stability_subsets(constraints)
        .into_iter()
        .filter_map(|subset| solve_once(&subset, hints, shared_rtt_floor_ms))
        .collect::<Vec<_>>();
    let samples = posterior_samples(mode, constraints, hints, shared_rtt_floor_ms);
    if samples.len() <= 1 {
        return PosteriorSummary {
            latitude: mode.latitude,
            longitude: mode.longitude,
            radius_km: 0.0,
        };
    }

    let mut deltas = samples
        .iter()
        .map(|candidate| (candidate.score - mode.score).max(0.0))
        .filter(|delta| *delta > 0.0)
        .collect::<Vec<_>>();
    deltas.sort_by(|left, right| left.total_cmp(right));
    let temperature = deltas
        .get(deltas.len().min(24).saturating_sub(1) / 2)
        .copied()
        .unwrap_or(1.0)
        .max(1.0);

    let weighted_samples = samples
        .into_iter()
        .map(|candidate| {
            let delta = (candidate.score - mode.score).max(0.0);
            let weight = (-delta / temperature).exp();
            (candidate, weight)
        })
        .collect::<Vec<_>>();
    let weighted_samples = normalize_weighted_candidates(weighted_samples, 0.5);
    let bootstrap_weighted_samples = normalize_weighted_candidates(
        bootstrap_modes
            .into_iter()
            .map(|candidate| (candidate, 1.0_f64))
            .collect(),
        0.5,
    );
    let weighted_samples = weighted_samples
        .into_iter()
        .chain(bootstrap_weighted_samples)
        .collect::<Vec<_>>();

    let (latitude, longitude) = weighted_candidate_centroid(&weighted_samples);
    let mut distances = weighted_samples
        .iter()
        .map(|(candidate, weight)| {
            (
                haversine_km(latitude, longitude, candidate.latitude, candidate.longitude),
                *weight,
            )
        })
        .collect::<Vec<_>>();
    distances.sort_by(|left, right| left.0.total_cmp(&right.0));
    let radius_km = weighted_quantile_distance(&distances, 0.75);

    PosteriorSummary {
        latitude,
        longitude,
        radius_km,
    }
}

fn posterior_samples(
    mode: Candidate,
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> Vec<Candidate> {
    let mut seeds = vec![(mode.latitude, mode.longitude)];
    seeds.extend(
        stability_subsets(constraints)
            .into_iter()
            .filter_map(|subset| solve_once(&subset, hints, shared_rtt_floor_ms))
            .map(|candidate| (candidate.latitude, candidate.longitude)),
    );
    seeds.extend(
        constraints
            .iter()
            .take(8)
            .map(|constraint| (constraint.anchor.latitude, constraint.anchor.longitude)),
    );
    seeds.extend(
        hints
            .iter()
            .map(|hint| (hint.anchor.latitude, hint.anchor.longitude)),
    );
    seeds.push(weighted_centroid(constraints));

    let mut seen = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();

    for (seed_latitude, seed_longitude) in seeds {
        for step in [2.0, 1.0, 0.5, 0.2, 0.1] {
            for lat_step in -4..=4 {
                for lon_step in -4..=4 {
                    let latitude = clamp_latitude(seed_latitude + f64::from(lat_step) * step);
                    let longitude =
                        normalize_longitude(seed_longitude + f64::from(lon_step) * step);
                    let key = (
                        (latitude * 1000.0).round() as i64,
                        (longitude * 1000.0).round() as i64,
                    );
                    if seen.insert(key) {
                        candidates.push(candidate(
                            latitude,
                            longitude,
                            constraints,
                            hints,
                            shared_rtt_floor_ms,
                        ));
                    }
                }
            }
        }
    }

    candidates
}

fn weighted_candidate_centroid(samples: &[(Candidate, f64)]) -> (f64, f64) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut total = 0.0;

    for (candidate, weight) in samples {
        let latitude = candidate.latitude.to_radians();
        let longitude = candidate.longitude.to_radians();
        x += weight * latitude.cos() * longitude.cos();
        y += weight * latitude.cos() * longitude.sin();
        z += weight * latitude.sin();
        total += weight;
    }

    if total == 0.0 {
        return (0.0, 0.0);
    }

    x /= total;
    y /= total;
    z /= total;

    let longitude = y.atan2(x);
    let hyp = (x.powi(2) + y.powi(2)).sqrt();
    let latitude = z.atan2(hyp);
    (latitude.to_degrees(), longitude.to_degrees())
}

fn weighted_quantile_distance(weighted_distances: &[(f64, f64)], quantile: f64) -> f64 {
    let total_weight = weighted_distances
        .iter()
        .map(|(_, weight)| *weight)
        .sum::<f64>();
    if total_weight <= 0.0 {
        return 0.0;
    }

    let threshold = total_weight * quantile.clamp(0.0, 1.0);
    let mut cumulative = 0.0;
    for (distance_km, weight) in weighted_distances {
        cumulative += *weight;
        if cumulative >= threshold {
            return *distance_km;
        }
    }

    weighted_distances
        .last()
        .map(|(distance_km, _)| *distance_km)
        .unwrap_or(0.0)
}

fn normalize_weighted_candidates(
    samples: Vec<(Candidate, f64)>,
    target_total_weight: f64,
) -> Vec<(Candidate, f64)> {
    let total_weight = samples.iter().map(|(_, weight)| *weight).sum::<f64>();
    if total_weight <= 0.0 || target_total_weight <= 0.0 {
        return samples;
    }

    samples
        .into_iter()
        .map(|(candidate, weight)| (candidate, weight * target_total_weight / total_weight))
        .collect()
}

fn dominant_local_anchor_score(
    latitude: f64,
    longitude: f64,
    constraints: &[Constraint],
    shared_rtt_floor_ms: f64,
) -> f64 {
    if shared_rtt_floor_ms <= 0.0 || constraints.len() < 2 {
        return 0.0;
    }

    let mut ranked = constraints.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.min_rtt_ms.total_cmp(&right.min_rtt_ms));

    let fastest = ranked[0];
    let second_fastest = ranked[1];
    let dominance_ratio = second_fastest.min_rtt_ms / fastest.min_rtt_ms.max(0.1);

    if fastest.min_rtt_ms > 10.0 || dominance_ratio < 3.0 {
        return 0.0;
    }

    let soft_radius_km =
        ((fastest.min_rtt_ms - shared_rtt_floor_ms).max(0.0) * MIN_KM_PER_MS * 0.05)
            .clamp(5.0, fastest.upper_bound_km.max(5.0));
    let distance_km = haversine_km(
        latitude,
        longitude,
        fastest.anchor.latitude,
        fastest.anchor.longitude,
    );
    let overflow_km = (distance_km - soft_radius_km).max(0.0);
    let strength = ((dominance_ratio - 3.0) / 3.0).clamp(0.0, 2.0)
        + if fastest.min_rtt_ms <= 2.0 { 1.0 } else { 0.0 };
    let center_bias = if fastest.min_rtt_ms <= 2.0 && dominance_ratio >= 5.0 {
        8.0 * strength * (distance_km / soft_radius_km.max(3.0)).powi(2)
    } else {
        0.0
    };

    60.0 * strength * (overflow_km / soft_radius_km.max(10.0)).powi(2) + center_bias
}

fn relative_spread_score(latitude: f64, longitude: f64, constraints: &[Constraint]) -> f64 {
    if constraints.len() < 2 {
        return 0.0;
    }

    let mut ranked = constraints.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.min_rtt_ms.total_cmp(&right.min_rtt_ms));

    let fastest = ranked[0];
    if fastest.min_rtt_ms > 10.0 {
        return 0.0;
    }

    let fastest_distance_km = haversine_km(
        latitude,
        longitude,
        fastest.anchor.latitude,
        fastest.anchor.longitude,
    );

    ranked
        .into_iter()
        .skip(1)
        .take(4)
        .filter(|constraint| constraint.min_rtt_ms <= fastest.min_rtt_ms + 40.0)
        .map(|constraint| {
            let delta_rtt_ms = constraint.min_rtt_ms - fastest.min_rtt_ms;
            if delta_rtt_ms < 2.0 {
                return 0.0;
            }

            let observed_delta_km = delta_rtt_ms * MIN_KM_PER_MS;
            let minimum_spread_km = (observed_delta_km * 0.30 - 20.0).max(0.0);
            if minimum_spread_km <= 0.0 {
                return 0.0;
            }

            let distance_km = haversine_km(
                latitude,
                longitude,
                constraint.anchor.latitude,
                constraint.anchor.longitude,
            );
            let predicted_spread_km = (distance_km - fastest_distance_km).max(0.0);
            let deficit_km = (minimum_spread_km - predicted_spread_km).max(0.0);
            let weight = (delta_rtt_ms / 10.0).clamp(0.5, 2.0)
                * fastest.quality_weight.min(constraint.quality_weight);

            45.0 * weight * (deficit_km / minimum_spread_km.max(100.0)).powi(2)
        })
        .sum()
}

fn pairwise_order_score(latitude: f64, longitude: f64, constraints: &[Constraint]) -> f64 {
    if constraints.len() < 2 {
        return 0.0;
    }

    let mut ranked = constraints.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.min_rtt_ms.total_cmp(&right.min_rtt_ms));
    let cutoff_ms = ranked[0].min_rtt_ms + 45.0;
    let local = ranked
        .into_iter()
        .filter(|constraint| constraint.min_rtt_ms <= cutoff_ms)
        .take(12)
        .collect::<Vec<_>>();

    let mut penalty = 0.0;
    for (left_index, faster) in local.iter().enumerate() {
        for slower in local.iter().skip(left_index + 1) {
            let observed_delta_ms = slower.min_rtt_ms - faster.min_rtt_ms;
            if observed_delta_ms < 1.5 {
                continue;
            }

            let effective_delta_ms =
                (observed_delta_ms - 0.5 * (faster.jitter_ms + slower.jitter_ms).min(8.0) - 1.0)
                    .max(0.0);
            if effective_delta_ms <= 0.0 {
                continue;
            }

            let faster_distance_km = haversine_km(
                latitude,
                longitude,
                faster.anchor.latitude,
                faster.anchor.longitude,
            );
            let slower_distance_km = haversine_km(
                latitude,
                longitude,
                slower.anchor.latitude,
                slower.anchor.longitude,
            );
            let required_advantage_km = (20.0 + 4.0 * effective_delta_ms).min(180.0);

            let predicted_advantage_km = slower_distance_km - faster_distance_km;
            let deficit_km = (required_advantage_km - predicted_advantage_km).max(0.0);
            let weight = 65.0
                * faster.quality_weight.min(slower.quality_weight)
                * (effective_delta_ms / 6.0).clamp(0.4, 2.0);

            penalty += weight * (deficit_km / required_advantage_km.max(120.0)).powi(2);

            let allowed_inversion_km = (100.0
                + 20.0 * (faster.jitter_ms + slower.jitter_ms).min(6.0)
                - 8.0 * observed_delta_ms)
                .clamp(40.0, 220.0);
            let inversion_km =
                (faster_distance_km - slower_distance_km - allowed_inversion_km).max(0.0);
            if inversion_km > 0.0 {
                let inversion_weight = 220.0
                    * faster.quality_weight.min(slower.quality_weight)
                    * (observed_delta_ms / 6.0).clamp(0.5, 3.0);
                penalty +=
                    inversion_weight * (inversion_km / allowed_inversion_km.max(120.0)).powi(2);
            }
        }
    }

    penalty
}

fn regional_cluster_score(latitude: f64, longitude: f64, constraints: &[Constraint]) -> f64 {
    let Some(cluster) = regional_cluster(constraints) else {
        return 0.0;
    };

    let distance_to_center_km = haversine_km(
        latitude,
        longitude,
        cluster.center_latitude,
        cluster.center_longitude,
    );
    let soft_radius_km = (cluster.span_km * 0.40).clamp(140.0, 650.0);
    let center_penalty =
        28.0 * cluster.strength * (distance_to_center_km / soft_radius_km.max(120.0)).powi(2);

    let fastest_distance_km = haversine_km(
        latitude,
        longitude,
        cluster.fastest.anchor.latitude,
        cluster.fastest.anchor.longitude,
    );
    let ordering_penalty = cluster
        .members
        .iter()
        .skip(1)
        .map(|constraint| {
            let observed_delta_ms = constraint.min_rtt_ms - cluster.fastest.min_rtt_ms;
            if observed_delta_ms <= 0.5 {
                return 0.0;
            }

            let distance_km = haversine_km(
                latitude,
                longitude,
                constraint.anchor.latitude,
                constraint.anchor.longitude,
            );
            let predicted_delta_km = (distance_km - fastest_distance_km).max(0.0);
            let allowed_delta_km = observed_delta_ms * 70.0
                + 140.0
                + 80.0 * (cluster.fastest.jitter_ms + constraint.jitter_ms).min(12.0);
            let overflow_km = (predicted_delta_km - allowed_delta_km).max(0.0);
            let weight = (observed_delta_ms / 6.0).clamp(0.5, 1.5)
                * cluster
                    .fastest
                    .quality_weight
                    .min(constraint.quality_weight);

            14.0 * cluster.strength * weight * (overflow_km / allowed_delta_km.max(150.0)).powi(2)
        })
        .sum::<f64>();

    center_penalty + ordering_penalty
}

#[derive(Debug, Clone)]
struct RegionalCluster<'a> {
    fastest: &'a Constraint,
    members: Vec<&'a Constraint>,
    center_latitude: f64,
    center_longitude: f64,
    span_km: f64,
    strength: f64,
}

fn regional_cluster<'a>(constraints: &'a [Constraint]) -> Option<RegionalCluster<'a>> {
    if constraints.len() < 3 {
        return None;
    }

    let mut ranked = constraints.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.min_rtt_ms.total_cmp(&right.min_rtt_ms));

    let fastest = ranked[0];
    let second_fastest = ranked[1];
    let dominance_ratio = second_fastest.min_rtt_ms / fastest.min_rtt_ms.max(0.1);
    if fastest.min_rtt_ms <= 12.0 && dominance_ratio >= 1.6 {
        return None;
    }

    let cutoff_ms = fastest.min_rtt_ms + 25.0;
    let members = ranked
        .into_iter()
        .take(5)
        .filter(|constraint| constraint.min_rtt_ms <= cutoff_ms)
        .collect::<Vec<_>>();
    if members.len() < 3 {
        return None;
    }

    let span_km = cluster_span_km(&members);
    if span_km > 2200.0 {
        return None;
    }

    let mean_quality = members
        .iter()
        .map(|constraint| constraint.quality_weight)
        .sum::<f64>()
        / members.len() as f64;
    let strength =
        ((members.len() as f64 - 2.0) * (1.45 - dominance_ratio).max(0.2) * mean_quality).min(2.5);
    if strength <= 0.0 {
        return None;
    }

    let (center_latitude, center_longitude) = weighted_anchor_centroid(&members);
    Some(RegionalCluster {
        fastest,
        members,
        center_latitude,
        center_longitude,
        span_km,
        strength,
    })
}

fn weighted_anchor_centroid(constraints: &[&Constraint]) -> (f64, f64) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut total = 0.0;

    for constraint in constraints {
        let weight = constraint.quality_weight / constraint.min_rtt_ms.max(5.0).powi(2);
        let lat = constraint.anchor.latitude.to_radians();
        let lon = constraint.anchor.longitude.to_radians();
        x += weight * lat.cos() * lon.cos();
        y += weight * lat.cos() * lon.sin();
        z += weight * lat.sin();
        total += weight;
    }

    if total == 0.0 {
        return (0.0, 0.0);
    }

    x /= total;
    y /= total;
    z /= total;

    let lon = y.atan2(x);
    let hyp = (x.powi(2) + y.powi(2)).sqrt();
    let lat = z.atan2(hyp);
    (lat.to_degrees(), lon.to_degrees())
}

fn cluster_span_km(constraints: &[&Constraint]) -> f64 {
    let mut span_km: f64 = 0.0;

    for (index, left) in constraints.iter().enumerate() {
        for right in constraints.iter().skip(index + 1) {
            span_km = span_km.max(haversine_km(
                left.anchor.latitude,
                left.anchor.longitude,
                right.anchor.latitude,
                right.anchor.longitude,
            ));
        }
    }

    span_km
}

fn stability_radius_km(
    best: Candidate,
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> f64 {
    let subsets = stability_subsets(constraints);
    let mut distances = subsets
        .into_iter()
        .filter_map(|subset| solve_once(&subset, hints, shared_rtt_floor_ms))
        .map(|candidate| {
            haversine_km(
                best.latitude,
                best.longitude,
                candidate.latitude,
                candidate.longitude,
            )
        })
        .collect::<Vec<_>>();

    if distances.is_empty() {
        return 0.0;
    }

    distances.sort_by(|left, right| left.total_cmp(right));
    let index = ((distances.len() - 1) * 3) / 4;
    distances[index]
}

fn stability_subsets(constraints: &[Constraint]) -> Vec<Vec<Constraint>> {
    let mut ranked = constraints.to_vec();
    ranked.sort_by(|left, right| left.min_rtt_ms.total_cmp(&right.min_rtt_ms));

    let mut subsets = Vec::new();

    for count in [4_usize, 5, 6, 8] {
        if ranked.len() >= count {
            subsets.push(ranked[..count].to_vec());
        }
    }

    let leave_one_out = ranked.len().min(4);
    for drop_index in 0..leave_one_out {
        let mut subset = ranked.clone();
        subset.remove(drop_index);
        if subset.len() >= 3 {
            subsets.push(subset);
        }
    }

    subsets
}

fn fit_latency_model(latitude: f64, longitude: f64, constraints: &[Constraint]) -> LatencyFit {
    let mut samples = constraints
        .iter()
        .map(|constraint| {
            let distance_km = haversine_km(
                latitude,
                longitude,
                constraint.anchor.latitude,
                constraint.anchor.longitude,
            );
            (constraint, distance_km)
        })
        .collect::<Vec<_>>();
    samples.sort_by(|left, right| left.0.min_rtt_ms.total_cmp(&right.0.min_rtt_ms));
    let fit_sample_count = samples.len().min(6);
    let fit_samples = &samples[..fit_sample_count];

    let mut sum_w = 0.0;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xy = 0.0;
    let mut distances = Vec::with_capacity(fit_samples.len());

    for (constraint, distance_km) in fit_samples.iter().copied() {
        let weight = constraint.quality_weight / constraint.min_rtt_ms.max(5.0).powi(2);
        distances.push(distance_km);
        sum_w += weight;
        sum_x += weight * distance_km;
        sum_y += weight * constraint.min_rtt_ms;
        sum_xx += weight * distance_km * distance_km;
        sum_xy += weight * distance_km * constraint.min_rtt_ms;
    }

    if sum_w == 0.0 {
        return LatencyFit {
            overhead_ms: 0.0,
            ms_per_km: MIN_MS_PER_KM,
            weighted_rmse_ms: 0.0,
        };
    }

    let denom = sum_w * sum_xx - sum_x * sum_x;
    let mut ms_per_km = if denom.abs() > f64::EPSILON {
        (sum_w * sum_xy - sum_x * sum_y) / denom
    } else {
        MIN_MS_PER_KM
    };
    ms_per_km = ms_per_km.max(MIN_MS_PER_KM);

    // Refit overhead after clamping to keep the predicted curve as close as possible.
    let overhead_ms = fit_samples
        .iter()
        .zip(distances.iter().copied())
        .map(|((constraint, _), distance_km)| {
            let weight = constraint.quality_weight / constraint.min_rtt_ms.max(5.0).powi(2);
            weight * (constraint.min_rtt_ms - ms_per_km * distance_km)
        })
        .sum::<f64>()
        / sum_w;
    let overhead_ms = overhead_ms.max(0.0);

    let weighted_mse = fit_samples
        .iter()
        .zip(distances.iter().copied())
        .map(|((constraint, _), distance_km)| {
            let weight = constraint.quality_weight / constraint.min_rtt_ms.max(5.0).powi(2);
            let predicted = overhead_ms + ms_per_km * distance_km;
            weight * (predicted - constraint.min_rtt_ms).powi(2)
        })
        .sum::<f64>()
        / sum_w;

    LatencyFit {
        overhead_ms,
        ms_per_km,
        weighted_rmse_ms: weighted_mse.sqrt(),
    }
}

fn clamp_latitude(latitude: f64) -> f64 {
    latitude.clamp(-89.9, 89.9)
}

fn normalize_longitude(longitude: f64) -> f64 {
    let wrapped = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped == -180.0 { 180.0 } else { wrapped }
}

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1 = lat1.to_radians();
    let lon1 = lon1.to_radians();
    let lat2 = lat2.to_radians();
    let lon2 = lon2.to_radians();

    let d_lat = lat2 - lat1;
    let d_lon = lon2 - lon1;

    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    EARTH_RADIUS_KM * c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::BUILTIN_ANCHORS;
    use crate::hints::LocationHint;
    use crate::probe::{Measurement, PingStats, ProbeMethod};

    fn anchor(id: &str) -> Anchor {
        BUILTIN_ANCHORS
            .iter()
            .copied()
            .find(|anchor| anchor.id == id)
            .expect("anchor should exist")
    }

    fn constraint(anchor: Anchor, upper_bound_km: f64, min_rtt_ms: f64) -> Constraint {
        Constraint {
            anchor,
            upper_bound_km,
            min_rtt_ms,
            jitter_ms: 0.0,
            quality_weight: 1.0,
        }
    }

    fn measurement(
        anchor_id: &str,
        min_ms: f64,
        avg_ms: f64,
        max_ms: f64,
        mdev_ms: f64,
    ) -> Measurement {
        Measurement {
            anchor: anchor(anchor_id),
            resolved_ip: None,
            ping: Some(PingStats {
                method: ProbeMethod::Icmp,
                transmitted: 1,
                received: 1,
                min_ms,
                avg_ms,
                max_ms,
                mdev_ms,
            }),
            trace: None,
            cache_age_days: None,
            cache_uncertainty_ms: 0.0,
            notes: Vec::new(),
        }
    }

    #[test]
    fn haversine_matches_expected_scale() {
        let paris_to_london = haversine_km(48.8566, 2.3522, 51.5072, -0.1276);
        assert!((paris_to_london - 343.0).abs() < 10.0);
    }

    #[test]
    fn solver_recovers_frankfurt_from_synthetic_constraints() {
        let target_lat = 50.1109;
        let target_lon = 8.6821;

        let anchors = [
            BUILTIN_ANCHORS[0],
            BUILTIN_ANCHORS[1],
            BUILTIN_ANCHORS[2],
            BUILTIN_ANCHORS[4],
            BUILTIN_ANCHORS[9],
        ];

        let constraints = anchors
            .iter()
            .map(|anchor| {
                let distance_km =
                    haversine_km(target_lat, target_lon, anchor.latitude, anchor.longitude);
                constraint(*anchor, distance_km + 150.0, (distance_km + 150.0) / 102.0)
            })
            .collect::<Vec<_>>();

        let estimate = solve(&constraints, &[], 0.0).expect("estimate should exist");
        let error_km = haversine_km(
            estimate.latitude,
            estimate.longitude,
            target_lat,
            target_lon,
        );

        assert!(error_km < 250.0, "error_km={error_km}");
    }

    #[test]
    fn hints_can_break_ties_toward_the_right_metro() {
        let constraints = vec![
            constraint(BUILTIN_ANCHORS[0], 400.0, 4.0),
            constraint(BUILTIN_ANCHORS[1], 400.0, 4.0),
        ];
        let hints = vec![LocationHint {
            anchor: BUILTIN_ANCHORS[0],
            weight: 8.0,
            source: "trace hop".to_string(),
            supporting_anchors: Vec::new(),
        }];

        let estimate = solve(&constraints, &hints, 0.0).expect("estimate should exist");
        let frankfurt_distance = haversine_km(
            estimate.mode_latitude,
            estimate.mode_longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );
        let zurich_distance = haversine_km(
            estimate.mode_latitude,
            estimate.mode_longitude,
            BUILTIN_ANCHORS[1].latitude,
            BUILTIN_ANCHORS[1].longitude,
        );

        assert!(frankfurt_distance < zurich_distance);
    }

    #[test]
    fn dominant_anchor_prior_pulls_estimate_toward_local_metro() {
        let constraints = vec![
            constraint(BUILTIN_ANCHORS[0], 53.0, 0.52),
            constraint(BUILTIN_ANCHORS[1], 689.0, 6.75),
            constraint(BUILTIN_ANCHORS[2], 1017.0, 9.97),
            constraint(BUILTIN_ANCHORS[5], 1379.0, 13.52),
        ];

        let raw_estimate = solve(&constraints, &[], 0.0).expect("raw estimate should exist");
        let guided_estimate = solve(&constraints, &[], 0.26).expect("guided estimate should exist");

        let raw_distance = haversine_km(
            raw_estimate.mode_latitude,
            raw_estimate.mode_longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );
        let guided_distance = haversine_km(
            guided_estimate.mode_latitude,
            guided_estimate.mode_longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );

        assert!(guided_distance < raw_distance);
    }

    #[test]
    fn regional_cluster_prior_improves_alpine_geometry() {
        let salzburg = (47.8095, 13.0550);
        let constraints = vec![
            constraint(anchor("eu-central-1"), 3372.0, 33.06),
            constraint(anchor("eu-central-2"), 3178.0, 31.15),
            constraint(anchor("eu-south-1"), 2920.0, 28.63),
            constraint(anchor("eu-south-2"), 5620.0, 55.10),
            constraint(anchor("eu-west-1"), 5681.0, 55.70),
            constraint(anchor("eu-west-2"), 4782.0, 46.88),
            constraint(anchor("eu-west-3"), 3693.0, 36.21),
            constraint(anchor("eu-north-1"), 5069.0, 49.70),
            constraint(anchor("il-central-1"), 8501.0, 83.35),
            constraint(anchor("us-east-1"), 11878.0, 116.45),
            constraint(anchor("ca-central-1"), 12150.0, 119.12),
            constraint(anchor("me-central-1"), 14203.0, 139.25),
            constraint(anchor("ap-south-1"), 15288.0, 149.88),
        ];

        let estimate = solve(&constraints, &[], 2.81).expect("estimate should exist");
        let error_km = haversine_km(
            estimate.latitude,
            estimate.longitude,
            salzburg.0,
            salzburg.1,
        );

        assert!(error_km < 250.0, "error_km={error_km}");
    }

    #[test]
    fn pairwise_ordering_prevents_slower_prague_from_beating_vienna() {
        let constraints = vec![
            constraint(anchor("at-vienna-interxion-1"), 1696.0, 16.63),
            constraint(anchor("at-vienna-1"), 1772.0, 17.38),
            Constraint {
                anchor: anchor("at-graz-1"),
                upper_bound_km: 2074.0,
                min_rtt_ms: 20.33,
                jitter_ms: 2.99,
                quality_weight: 0.55,
            },
            constraint(anchor("munich-de-cix"), 2076.0, 20.36),
            constraint(anchor("hr-zagreb-1"), 2546.0, 24.96),
            constraint(anchor("si-ljubljana-1"), 2595.0, 25.44),
            Constraint {
                anchor: anchor("cz-prague-1"),
                upper_bound_km: 3508.0,
                min_rtt_ms: 34.40,
                jitter_ms: 4.91,
                quality_weight: 0.45,
            },
            constraint(anchor("hu-budapest-1"), 3979.0, 39.01),
        ];

        let estimate = solve(&constraints, &[], 1.72).expect("estimate should exist");
        let prague_distance = haversine_km(
            estimate.latitude,
            estimate.longitude,
            anchor("cz-prague-1").latitude,
            anchor("cz-prague-1").longitude,
        );
        let vienna_distance = haversine_km(
            estimate.latitude,
            estimate.longitude,
            anchor("at-vienna-1").latitude,
            anchor("at-vienna-1").longitude,
        );

        assert!(vienna_distance < prague_distance);
    }

    #[test]
    fn salzburg_style_trace_hints_pull_solution_out_of_prague() {
        let salzburg = (47.8095, 13.0550);
        let measurements = vec![
            measurement("at-vienna-interxion-1", 16.63, 24.11, 31.13, 5.93),
            measurement("at-vienna-1", 17.38, 17.38, 17.38, 0.0),
            measurement("at-graz-1", 20.33, 24.41, 27.41, 2.99),
            measurement("munich-de-cix", 20.36, 20.36, 20.36, 0.0),
            measurement("eu-central-1", 24.87, 24.87, 24.87, 0.0),
            measurement("hr-zagreb-1", 24.96, 24.96, 24.96, 0.0),
            measurement("si-ljubljana-1", 25.44, 25.44, 25.44, 0.0),
            measurement("eu-south-1", 27.19, 33.03, 43.03, 7.10),
            measurement("eu-central-2", 30.30, 30.30, 30.30, 0.0),
            measurement("cz-prague-1", 34.40, 41.33, 45.15, 4.91),
            measurement("eu-west-3", 36.21, 36.21, 36.21, 0.0),
            measurement("hu-budapest-1", 39.01, 39.01, 39.01, 0.0),
            measurement("eu-west-2", 39.85, 39.85, 39.85, 0.0),
        ];
        let constraints = constraints_from_measurements(&measurements, 102.0);
        let hints = vec![LocationHint {
            anchor: anchor("at-vienna-1"),
            weight: 8.0,
            source: "trace hop hostname".to_string(),
            supporting_anchors: vec![anchor("at-vienna-1"), anchor("at-graz-1")],
        }];

        let estimate = solve(&constraints, &hints, 1.72).expect("estimate should exist");
        let error_km = haversine_km(
            estimate.latitude,
            estimate.longitude,
            salzburg.0,
            salzburg.1,
        );

        assert!(
            error_km < 100.0,
            "estimate=({:.4}, {:.4}) error_km={error_km}",
            estimate.latitude,
            estimate.longitude,
        );
    }

    #[test]
    fn high_overhead_austria_replay_reports_broad_alpine_posterior() {
        let salzburg = (47.8095, 13.0550);
        let measurements = vec![
            measurement("eu-central-1", 27.78, 27.78, 27.78, 0.0),
            measurement("eu-central-2", 32.10, 38.52, 44.31, 5.01),
            measurement("eu-south-1", 30.18, 30.18, 30.18, 0.0),
            measurement("eu-south-2", 62.13, 62.13, 62.13, 0.0),
            measurement("eu-west-1", 68.43, 68.43, 68.43, 0.0),
            measurement("eu-west-2", 44.62, 44.62, 44.62, 0.0),
            measurement("eu-west-3", 39.20, 39.20, 39.20, 0.0),
            measurement("eu-north-1", 51.04, 51.04, 51.04, 0.0),
            measurement("us-east-1", 107.04, 107.04, 107.04, 0.0),
            measurement("ca-central-1", 115.17, 115.17, 115.17, 0.0),
            measurement("us-west-2", 178.38, 178.38, 178.38, 0.0),
            measurement("ap-south-1", 155.27, 155.27, 155.27, 0.0),
            measurement("ap-south-2", 163.07, 163.07, 163.07, 0.0),
            measurement("ap-southeast-1", 267.56, 267.56, 267.56, 0.0),
            measurement("ap-southeast-2", 299.67, 299.67, 299.67, 0.0),
            measurement("ap-east-1", 280.74, 280.74, 280.74, 0.0),
            measurement("ap-northeast-1", 263.57, 263.57, 263.57, 0.0),
            measurement("ap-northeast-2", 277.00, 277.00, 277.00, 0.0),
            measurement("il-central-1", 89.09, 89.09, 89.09, 0.0),
            measurement("me-central-1", 142.06, 142.06, 142.06, 0.0),
            measurement("sa-east-1", 226.18, 226.18, 226.18, 0.0),
            measurement("mx-central-1", 162.79, 162.79, 162.79, 0.0),
            measurement("af-south-1", 227.78, 227.78, 227.78, 0.0),
            measurement("munich-de-cix", 41.93, 41.93, 41.93, 0.0),
            measurement("at-vienna-1", 16.66, 17.71, 18.89, 0.91),
            measurement("at-vienna-interxion-1", 15.85, 20.86, 24.07, 3.59),
            measurement("at-graz-1", 22.55, 22.55, 22.55, 0.0),
            measurement("cz-prague-1", 34.38, 34.38, 34.38, 0.0),
            measurement("hu-budapest-1", 38.89, 38.89, 38.89, 0.0),
            measurement("si-ljubljana-1", 23.71, 25.82, 28.19, 1.84),
            measurement("hr-zagreb-1", 24.70, 26.97, 30.37, 2.45),
        ];
        let constraints = constraints_from_measurements(&measurements, 102.0);
        let hints = vec![LocationHint {
            anchor: anchor("at-vienna-1"),
            weight: 6.0,
            source: "Vienna trace corridor".to_string(),
            supporting_anchors: vec![anchor("at-vienna-1"), anchor("at-graz-1")],
        }];

        let estimate = solve(&constraints, &hints, 2.74).expect("estimate should exist");
        let error_km = haversine_km(
            estimate.latitude,
            estimate.longitude,
            salzburg.0,
            salzburg.1,
        );

        assert!(error_km < 170.0, "error_km={error_km}");
        assert!(estimate.posterior_radius_km > 100.0);
        assert!(estimate.confidence_radius_km > 200.0);
    }
}
