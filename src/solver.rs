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
    pub fitted_overhead_ms: f64,
    pub fitted_km_per_ms: f64,
    pub weighted_rmse_ms: f64,
    pub confidence_radius_km: f64,
    pub constraints: Vec<EvaluatedConstraint>,
    pub hints: Vec<EvaluatedHint>,
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
            })
        })
        .collect()
}

pub fn solve(
    constraints: &[Constraint],
    hints: &[LocationHint],
    shared_rtt_floor_ms: f64,
) -> Option<Estimate> {
    let best = solve_once(constraints, hints, shared_rtt_floor_ms)?;
    let confidence_radius_km = stability_radius_km(best, constraints, hints, shared_rtt_floor_ms);

    Some(build_estimate(
        best,
        constraints,
        hints,
        confidence_radius_km,
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
    constraints: &[Constraint],
    hints: &[LocationHint],
    confidence_radius_km: f64,
) -> Estimate {
    Estimate {
        latitude: best.latitude,
        longitude: best.longitude,
        score: best.score,
        fitted_overhead_ms: best.fit.overhead_ms,
        fitted_km_per_ms: 1.0 / best.fit.ms_per_km,
        weighted_rmse_ms: best.fit.weighted_rmse_ms,
        confidence_radius_km,
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
            1000.0 * (overflow_km / scale).powi(2)
        })
        .sum();

    let hint_score: f64 = hints
        .iter()
        .map(|hint| {
            let distance_km = haversine_km(
                latitude,
                longitude,
                hint.anchor.latitude,
                hint.anchor.longitude,
            );
            hint.weight * distance_km / 150.0
        })
        .sum();

    let dominant_anchor_score =
        dominant_local_anchor_score(latitude, longitude, constraints, shared_rtt_floor_ms);
    let relative_spread_score = relative_spread_score(latitude, longitude, constraints);
    let regional_cluster_score = regional_cluster_score(latitude, longitude, constraints);

    let fit_penalty = 80.0 * fit.weighted_rmse_ms
        + 0.5 * fit.overhead_ms
        + 2000.0 * (fit.ms_per_km - MIN_MS_PER_KM).max(0.0);

    upper_bound_score
        + fit_penalty
        + hint_score
        + dominant_anchor_score
        + relative_spread_score
        + regional_cluster_score
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
        .filter(|constraint| constraint.min_rtt_ms <= 50.0)
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
            let weight = (delta_rtt_ms / 10.0).clamp(0.5, 2.0);

            45.0 * weight * (deficit_km / minimum_spread_km.max(100.0)).powi(2)
        })
        .sum()
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
        18.0 * cluster.strength * (distance_to_center_km / soft_radius_km.max(120.0)).powi(2);

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
            let allowed_delta_km = observed_delta_ms * 85.0 + 140.0;
            let overflow_km = (predicted_delta_km - allowed_delta_km).max(0.0);
            let weight = (observed_delta_ms / 6.0).clamp(0.5, 1.5);

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

    let strength = ((members.len() as f64 - 2.0) * (1.45 - dominance_ratio).max(0.2)).min(2.5);
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
        let weight = 1.0 / constraint.min_rtt_ms.max(5.0).powi(2);
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
        let weight = 1.0 / constraint.min_rtt_ms.max(5.0).powi(2);
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
            let weight = 1.0 / constraint.min_rtt_ms.max(5.0).powi(2);
            weight * (constraint.min_rtt_ms - ms_per_km * distance_km)
        })
        .sum::<f64>()
        / sum_w;
    let overhead_ms = overhead_ms.max(0.0);

    let weighted_mse = fit_samples
        .iter()
        .zip(distances.iter().copied())
        .map(|((constraint, _), distance_km)| {
            let weight = 1.0 / constraint.min_rtt_ms.max(5.0).powi(2);
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

    fn anchor(id: &str) -> Anchor {
        BUILTIN_ANCHORS
            .iter()
            .copied()
            .find(|anchor| anchor.id == id)
            .expect("anchor should exist")
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
                Constraint {
                    anchor: *anchor,
                    upper_bound_km: distance_km + 150.0,
                    min_rtt_ms: (distance_km + 150.0) / 102.0,
                }
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
            Constraint {
                anchor: BUILTIN_ANCHORS[0],
                upper_bound_km: 400.0,
                min_rtt_ms: 4.0,
            },
            Constraint {
                anchor: BUILTIN_ANCHORS[1],
                upper_bound_km: 400.0,
                min_rtt_ms: 4.0,
            },
        ];
        let hints = vec![LocationHint {
            anchor: BUILTIN_ANCHORS[0],
            weight: 8.0,
            source: "trace hop".to_string(),
        }];

        let estimate = solve(&constraints, &hints, 0.0).expect("estimate should exist");
        let frankfurt_distance = haversine_km(
            estimate.latitude,
            estimate.longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );
        let zurich_distance = haversine_km(
            estimate.latitude,
            estimate.longitude,
            BUILTIN_ANCHORS[1].latitude,
            BUILTIN_ANCHORS[1].longitude,
        );

        assert!(frankfurt_distance < zurich_distance);
    }

    #[test]
    fn dominant_anchor_prior_pulls_estimate_toward_local_metro() {
        let constraints = vec![
            Constraint {
                anchor: BUILTIN_ANCHORS[0],
                upper_bound_km: 53.0,
                min_rtt_ms: 0.52,
            },
            Constraint {
                anchor: BUILTIN_ANCHORS[1],
                upper_bound_km: 689.0,
                min_rtt_ms: 6.75,
            },
            Constraint {
                anchor: BUILTIN_ANCHORS[2],
                upper_bound_km: 1017.0,
                min_rtt_ms: 9.97,
            },
            Constraint {
                anchor: BUILTIN_ANCHORS[5],
                upper_bound_km: 1379.0,
                min_rtt_ms: 13.52,
            },
        ];

        let raw_estimate = solve(&constraints, &[], 0.0).expect("raw estimate should exist");
        let guided_estimate = solve(&constraints, &[], 0.26).expect("guided estimate should exist");

        let raw_distance = haversine_km(
            raw_estimate.latitude,
            raw_estimate.longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );
        let guided_distance = haversine_km(
            guided_estimate.latitude,
            guided_estimate.longitude,
            BUILTIN_ANCHORS[0].latitude,
            BUILTIN_ANCHORS[0].longitude,
        );

        assert!(guided_distance < raw_distance);
    }

    #[test]
    fn regional_cluster_prior_improves_alpine_geometry() {
        let salzburg = (47.8095, 13.0550);
        let constraints = vec![
            Constraint {
                anchor: anchor("eu-central-1"),
                upper_bound_km: 3372.0,
                min_rtt_ms: 33.06,
            },
            Constraint {
                anchor: anchor("eu-central-2"),
                upper_bound_km: 3178.0,
                min_rtt_ms: 31.15,
            },
            Constraint {
                anchor: anchor("eu-south-1"),
                upper_bound_km: 2920.0,
                min_rtt_ms: 28.63,
            },
            Constraint {
                anchor: anchor("eu-south-2"),
                upper_bound_km: 5620.0,
                min_rtt_ms: 55.10,
            },
            Constraint {
                anchor: anchor("eu-west-1"),
                upper_bound_km: 5681.0,
                min_rtt_ms: 55.70,
            },
            Constraint {
                anchor: anchor("eu-west-2"),
                upper_bound_km: 4782.0,
                min_rtt_ms: 46.88,
            },
            Constraint {
                anchor: anchor("eu-west-3"),
                upper_bound_km: 3693.0,
                min_rtt_ms: 36.21,
            },
            Constraint {
                anchor: anchor("eu-north-1"),
                upper_bound_km: 5069.0,
                min_rtt_ms: 49.70,
            },
            Constraint {
                anchor: anchor("il-central-1"),
                upper_bound_km: 8501.0,
                min_rtt_ms: 83.35,
            },
            Constraint {
                anchor: anchor("us-east-1"),
                upper_bound_km: 11878.0,
                min_rtt_ms: 116.45,
            },
            Constraint {
                anchor: anchor("ca-central-1"),
                upper_bound_km: 12150.0,
                min_rtt_ms: 119.12,
            },
            Constraint {
                anchor: anchor("me-central-1"),
                upper_bound_km: 14203.0,
                min_rtt_ms: 139.25,
            },
            Constraint {
                anchor: anchor("ap-south-1"),
                upper_bound_km: 15288.0,
                min_rtt_ms: 149.88,
            },
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
}
