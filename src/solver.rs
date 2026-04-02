use crate::anchors::Anchor;
use crate::probe::Measurement;

const EARTH_RADIUS_KM: f64 = 6371.0;

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
pub struct Estimate {
    pub latitude: f64,
    pub longitude: f64,
    pub score: f64,
    pub constraints: Vec<EvaluatedConstraint>,
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

pub fn solve(constraints: &[Constraint]) -> Option<Estimate> {
    if constraints.is_empty() {
        return None;
    }

    let mut best = initial_candidates(constraints)
        .into_iter()
        .map(|(lat, lon)| candidate(lat, lon, constraints))
        .min_by(|left, right| left.score.total_cmp(&right.score))?;

    for step in [8.0, 2.0, 0.5, 0.1, 0.02] {
        best = refine(best.latitude, best.longitude, step, constraints, best.score);
    }

    Some(Estimate {
        latitude: best.latitude,
        longitude: best.longitude,
        score: best.score,
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
    })
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    latitude: f64,
    longitude: f64,
    score: f64,
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
    current_score: f64,
) -> Candidate {
    let mut best = Candidate {
        latitude,
        longitude,
        score: current_score,
    };

    for lat_step in -8..=8 {
        for lon_step in -8..=8 {
            let next_lat = clamp_latitude(latitude + f64::from(lat_step) * step);
            let next_lon = normalize_longitude(longitude + f64::from(lon_step) * step);
            let candidate = candidate(next_lat, next_lon, constraints);
            if candidate.score < best.score {
                best = candidate;
            }
        }
    }

    best
}

fn candidate(latitude: f64, longitude: f64, constraints: &[Constraint]) -> Candidate {
    Candidate {
        latitude,
        longitude,
        score: objective(latitude, longitude, constraints),
    }
}

fn objective(latitude: f64, longitude: f64, constraints: &[Constraint]) -> f64 {
    constraints
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
            let overflow_penalty = 500.0 * (overflow_km / scale).powi(2);
            let attraction = 25.0 * distance_km / scale.powf(1.25);
            overflow_penalty + attraction
        })
        .sum()
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

        let estimate = solve(&constraints).expect("estimate should exist");
        let error_km = haversine_km(
            estimate.latitude,
            estimate.longitude,
            target_lat,
            target_lon,
        );

        assert!(error_km < 250.0, "error_km={error_km}");
    }
}
