use std::collections::BTreeMap;
use std::process::Command;

use crate::anchors::{Anchor, BUILTIN_ANCHORS};
use crate::probe::Measurement;

#[derive(Debug, Clone)]
pub struct LocationHint {
    pub anchor: Anchor,
    pub weight: f64,
    pub source: String,
}

pub fn derive_trace_location_hints(measurements: &[Measurement]) -> Vec<LocationHint> {
    let mut weights: BTreeMap<&'static str, (Anchor, f64, Vec<String>)> = BTreeMap::new();

    for measurement in measurements {
        let Some(trace) = &measurement.trace else {
            continue;
        };

        for hop in trace.hops.iter().take(6) {
            let hop_weight = match hop.hop {
                1 => 5.0,
                2 => 4.0,
                3 => 3.0,
                4 => 2.0,
                _ => 1.0,
            };

            if let Some(hostname) = &hop.hostname {
                accumulate_matches(
                    &mut weights,
                    hostname,
                    hop_weight,
                    format!(
                        "{} hop {} hostname={hostname}",
                        measurement.anchor.label(),
                        hop.hop
                    ),
                );
            }
        }
    }

    weights_to_hints(weights)
}

pub fn derive_location_hints(measurements: &[Measurement]) -> Vec<LocationHint> {
    let mut weights: BTreeMap<&'static str, (Anchor, f64, Vec<String>)> = BTreeMap::new();

    if let Some(hostname) = local_hostname() {
        accumulate_matches(
            &mut weights,
            &hostname,
            1.5,
            format!("local hostname={hostname}"),
        );
    }

    for measurement in measurements {
        if let Some(trace) = &measurement.trace {
            for hop in trace.hops.iter().take(6) {
                let hop_weight = match hop.hop {
                    1 => 5.0,
                    2 => 4.0,
                    3 => 3.0,
                    4 => 2.0,
                    _ => 1.0,
                };

                if let Some(hostname) = &hop.hostname {
                    accumulate_matches(
                        &mut weights,
                        hostname,
                        hop_weight,
                        format!(
                            "{} hop {} hostname={hostname}",
                            measurement.anchor.label(),
                            hop.hop
                        ),
                    );
                }
            }
        }
    }

    weights_to_hints(weights)
}

fn weights_to_hints(
    weights: BTreeMap<&'static str, (Anchor, f64, Vec<String>)>,
) -> Vec<LocationHint> {
    weights
        .into_values()
        .map(|(anchor, weight, sources)| LocationHint {
            anchor,
            weight: weight.min(12.0),
            source: sources.join(" | "),
        })
        .collect()
}

fn accumulate_matches(
    weights: &mut BTreeMap<&'static str, (Anchor, f64, Vec<String>)>,
    text: &str,
    weight: f64,
    source: String,
) {
    let haystack = text.to_lowercase();
    let tokens = tokenize(&haystack);

    for alias in ALIASES {
        let matched = if alias.partial {
            haystack.contains(alias.token)
        } else {
            tokens.iter().any(|token| token == alias.token)
        };

        if !matched {
            continue;
        }

        let Some(anchor) = BUILTIN_ANCHORS
            .iter()
            .copied()
            .find(|anchor| anchor.id == alias.id)
        else {
            continue;
        };

        let entry = weights
            .entry(alias.id)
            .or_insert_with(|| (anchor, 0.0, Vec::new()));
        entry.1 += weight;
        entry.2.push(source.clone());
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn local_hostname() -> Option<String> {
    let output = Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let hostname = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hostname.is_empty() {
        None
    } else {
        Some(hostname)
    }
}

#[derive(Clone, Copy)]
struct Alias {
    token: &'static str,
    id: &'static str,
    partial: bool,
}

const ALIASES: &[Alias] = &[
    Alias {
        token: "fra",
        id: "eu-central-1",
        partial: false,
    },
    Alias {
        token: "frankfurt",
        id: "eu-central-1",
        partial: true,
    },
    Alias {
        token: "ffm",
        id: "eu-central-1",
        partial: false,
    },
    Alias {
        token: "zrh",
        id: "eu-central-2",
        partial: false,
    },
    Alias {
        token: "zurich",
        id: "eu-central-2",
        partial: true,
    },
    Alias {
        token: "mil",
        id: "eu-south-1",
        partial: false,
    },
    Alias {
        token: "milan",
        id: "eu-south-1",
        partial: true,
    },
    Alias {
        token: "mad",
        id: "eu-south-2",
        partial: false,
    },
    Alias {
        token: "madrid",
        id: "eu-south-2",
        partial: true,
    },
    Alias {
        token: "dub",
        id: "eu-west-1",
        partial: false,
    },
    Alias {
        token: "dublin",
        id: "eu-west-1",
        partial: true,
    },
    Alias {
        token: "par",
        id: "eu-west-3",
        partial: false,
    },
    Alias {
        token: "paris",
        id: "eu-west-3",
        partial: true,
    },
    Alias {
        token: "arn",
        id: "eu-north-1",
        partial: false,
    },
    Alias {
        token: "sto",
        id: "eu-north-1",
        partial: false,
    },
    Alias {
        token: "stockholm",
        id: "eu-north-1",
        partial: true,
    },
    Alias {
        token: "iad",
        id: "us-east-1",
        partial: false,
    },
    Alias {
        token: "ashburn",
        id: "us-east-1",
        partial: true,
    },
    Alias {
        token: "yul",
        id: "ca-central-1",
        partial: false,
    },
    Alias {
        token: "montreal",
        id: "ca-central-1",
        partial: true,
    },
    Alias {
        token: "pdx",
        id: "us-west-2",
        partial: false,
    },
    Alias {
        token: "portland",
        id: "us-west-2",
        partial: true,
    },
    Alias {
        token: "bom",
        id: "ap-south-1",
        partial: false,
    },
    Alias {
        token: "mumbai",
        id: "ap-south-1",
        partial: true,
    },
    Alias {
        token: "hyd",
        id: "ap-south-2",
        partial: false,
    },
    Alias {
        token: "hyderabad",
        id: "ap-south-2",
        partial: true,
    },
    Alias {
        token: "sin",
        id: "ap-southeast-1",
        partial: false,
    },
    Alias {
        token: "sgp",
        id: "ap-southeast-1",
        partial: false,
    },
    Alias {
        token: "singapore",
        id: "ap-southeast-1",
        partial: true,
    },
    Alias {
        token: "syd",
        id: "ap-southeast-2",
        partial: false,
    },
    Alias {
        token: "sydney",
        id: "ap-southeast-2",
        partial: true,
    },
    Alias {
        token: "hkg",
        id: "ap-east-1",
        partial: false,
    },
    Alias {
        token: "hongkong",
        id: "ap-east-1",
        partial: true,
    },
    Alias {
        token: "hong-kong",
        id: "ap-east-1",
        partial: true,
    },
    Alias {
        token: "nrt",
        id: "ap-northeast-1",
        partial: false,
    },
    Alias {
        token: "hnd",
        id: "ap-northeast-1",
        partial: false,
    },
    Alias {
        token: "tokyo",
        id: "ap-northeast-1",
        partial: true,
    },
    Alias {
        token: "tlv",
        id: "il-central-1",
        partial: false,
    },
    Alias {
        token: "telaviv",
        id: "il-central-1",
        partial: true,
    },
    Alias {
        token: "abudhabi",
        id: "me-central-1",
        partial: true,
    },
    Alias {
        token: "auh",
        id: "me-central-1",
        partial: false,
    },
    Alias {
        token: "gru",
        id: "sa-east-1",
        partial: false,
    },
    Alias {
        token: "saopaulo",
        id: "sa-east-1",
        partial: true,
    },
    Alias {
        token: "cpt",
        id: "af-south-1",
        partial: false,
    },
    Alias {
        token: "capetown",
        id: "af-south-1",
        partial: true,
    },
    Alias {
        token: "qro",
        id: "mx-central-1",
        partial: false,
    },
    Alias {
        token: "queretaro",
        id: "mx-central-1",
        partial: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Measurement, TraceHop, TraceSummary};

    #[test]
    fn extracts_frankfurt_hint_from_hostname() {
        let mut weights = BTreeMap::new();
        accumulate_matches(
            &mut weights,
            "core1.de-cix-fra.digitalocean.net",
            5.0,
            "trace".to_string(),
        );

        let (anchor, weight, _) = weights.get("eu-central-1").expect("hint should exist");
        assert_eq!(anchor.metro, "Frankfurt");
        assert!(*weight >= 5.0);
    }

    #[test]
    fn ignores_unrelated_text() {
        let mut weights = BTreeMap::new();
        accumulate_matches(
            &mut weights,
            "router.edge.example.net",
            5.0,
            "trace".to_string(),
        );

        assert!(weights.is_empty());
    }

    #[test]
    fn derives_trace_only_hints_from_trace_hostnames() {
        let hints = derive_trace_location_hints(&[Measurement {
            anchor: BUILTIN_ANCHORS[0],
            resolved_ip: None,
            ping: None,
            trace: Some(TraceSummary {
                hops: vec![TraceHop {
                    hop: 2,
                    address: Some("192.0.2.1".to_string()),
                    hostname: Some("core1.de-cix-fra.digitalocean.net".to_string()),
                    rtt_ms: Some(0.8),
                    note: None,
                }],
            }),
            notes: Vec::new(),
        }]);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].anchor.id, "eu-central-1");
        assert!(hints[0].weight >= 4.0);
    }
}
