use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use crate::anchors::{Anchor, BUILTIN_ANCHORS};
use crate::probe::Measurement;

type SupportingAnchorIds = BTreeSet<&'static str>;
type HintAccumulator = (Anchor, f64, Vec<String>, SupportingAnchorIds);
type HintWeights = BTreeMap<&'static str, HintAccumulator>;

#[derive(Debug, Clone)]
pub struct LocationHint {
    pub anchor: Anchor,
    pub weight: f64,
    pub source: String,
    pub supporting_anchors: Vec<Anchor>,
}

pub fn derive_trace_location_hints(measurements: &[Measurement]) -> Vec<LocationHint> {
    let mut weights: HintWeights = BTreeMap::new();

    for measurement in measurements {
        let Some(trace) = &measurement.trace else {
            continue;
        };

        for hop in trace.hops.iter().take(10) {
            let hop_weight = match hop.hop {
                1 => 5.0,
                2 => 4.0,
                3 => 3.0,
                4 => 2.0,
                5 | 6 => 1.5,
                7 | 8 => 2.0,
                _ => 1.0,
            };

            if let Some(hostname) = &hop.hostname {
                accumulate_matches(
                    &mut weights,
                    hostname,
                    hop_weight,
                    Some(measurement.anchor),
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
    let mut weights: HintWeights = BTreeMap::new();

    if let Some(hostname) = local_hostname() {
        accumulate_matches(
            &mut weights,
            &hostname,
            1.5,
            None,
            format!("local hostname={hostname}"),
        );
    }

    for measurement in measurements {
        if let Some(trace) = &measurement.trace {
            for hop in trace.hops.iter().take(10) {
                let hop_weight = match hop.hop {
                    1 => 5.0,
                    2 => 4.0,
                    3 => 3.0,
                    4 => 2.0,
                    5 | 6 => 1.5,
                    7 | 8 => 2.0,
                    _ => 1.0,
                };

                if let Some(hostname) = &hop.hostname {
                    accumulate_matches(
                        &mut weights,
                        hostname,
                        hop_weight,
                        Some(measurement.anchor),
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

fn weights_to_hints(weights: HintWeights) -> Vec<LocationHint> {
    weights
        .into_values()
        .map(
            |(anchor, weight, sources, supporting_anchor_ids)| LocationHint {
                anchor,
                weight: weight.min(12.0),
                source: sources.join(" | "),
                supporting_anchors: supporting_anchor_ids
                    .into_iter()
                    .filter_map(|anchor_id| {
                        BUILTIN_ANCHORS
                            .iter()
                            .copied()
                            .find(|candidate| candidate.id == anchor_id)
                    })
                    .collect(),
            },
        )
        .collect()
}

fn accumulate_matches(
    weights: &mut HintWeights,
    text: &str,
    weight: f64,
    supporting_anchor: Option<Anchor>,
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
            .or_insert_with(|| (anchor, 0.0, Vec::new(), BTreeSet::new()));
        entry.1 += weight;
        entry.2.push(source.clone());
        if let Some(supporting_anchor) = supporting_anchor {
            entry.3.insert(supporting_anchor.id);
        }
    }
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = BTreeSet::new();

    for raw_token in text.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if raw_token.is_empty() {
            continue;
        }

        tokens.insert(raw_token.to_string());

        let alpha_prefix = raw_token
            .chars()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .collect::<String>();
        if alpha_prefix.len() >= 3 && alpha_prefix.len() < raw_token.len() {
            tokens.insert(alpha_prefix);
        }

        let alpha_suffix = raw_token
            .chars()
            .rev()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        if alpha_suffix.len() >= 3 && alpha_suffix.len() < raw_token.len() {
            tokens.insert(alpha_suffix);
        }
    }

    tokens.into_iter().collect()
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
        token: "muc",
        id: "munich-de-cix",
        partial: false,
    },
    Alias {
        token: "munich",
        id: "munich-de-cix",
        partial: true,
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
        token: "prg",
        id: "cz-prague-1",
        partial: false,
    },
    Alias {
        token: "prague",
        id: "cz-prague-1",
        partial: true,
    },
    Alias {
        token: "vie",
        id: "at-vienna-1",
        partial: false,
    },
    Alias {
        token: "vix",
        id: "at-vienna-1",
        partial: true,
    },
    Alias {
        token: "vienna",
        id: "at-vienna-1",
        partial: true,
    },
    Alias {
        token: "grz",
        id: "at-graz-1",
        partial: false,
    },
    Alias {
        token: "graz",
        id: "at-graz-1",
        partial: true,
    },
    Alias {
        token: "bud",
        id: "hu-budapest-1",
        partial: false,
    },
    Alias {
        token: "budapest",
        id: "hu-budapest-1",
        partial: true,
    },
    Alias {
        token: "lju",
        id: "si-ljubljana-1",
        partial: false,
    },
    Alias {
        token: "ljubljana",
        id: "si-ljubljana-1",
        partial: true,
    },
    Alias {
        token: "zag",
        id: "hr-zagreb-1",
        partial: false,
    },
    Alias {
        token: "zagreb",
        id: "hr-zagreb-1",
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
            None,
            "trace".to_string(),
        );

        let (anchor, weight, _, _) = weights.get("eu-central-1").expect("hint should exist");
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
            None,
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
            cache_age_days: None,
            cache_uncertainty_ms: 0.0,
            notes: Vec::new(),
        }]);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].anchor.id, "eu-central-1");
        assert!(hints[0].weight >= 4.0);
        assert_eq!(hints[0].supporting_anchors[0].id, "eu-central-1");
    }

    #[test]
    fn derives_vienna_hint_from_later_trace_hops() {
        let hints = derive_trace_location_hints(&[Measurement {
            anchor: BUILTIN_ANCHORS[0],
            resolved_ip: None,
            ping: None,
            trace: Some(TraceSummary {
                hops: vec![TraceHop {
                    hop: 8,
                    address: Some("77.244.255.130".to_string()),
                    hostname: Some("ae10.edge01.ndc2.vie.nessus.at".to_string()),
                    rtt_ms: Some(23.6),
                    note: None,
                }],
            }),
            cache_age_days: None,
            cache_uncertainty_ms: 0.0,
            notes: Vec::new(),
        }]);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].anchor.id, "at-vienna-1");
        assert_eq!(hints[0].supporting_anchors[0].id, "eu-central-1");
    }

    #[test]
    fn derives_vienna_hint_from_embedded_code_token() {
        let hints = derive_trace_location_hints(&[Measurement {
            anchor: BUILTIN_ANCHORS[0],
            resolved_ip: None,
            ping: None,
            trace: Some(TraceSummary {
                hops: vec![TraceHop {
                    hop: 7,
                    address: Some("193.203.0.85".to_string()),
                    hostname: Some("at-vie09c-ri01.as8412.net".to_string()),
                    rtt_ms: Some(23.5),
                    note: None,
                }],
            }),
            cache_age_days: None,
            cache_uncertainty_ms: 0.0,
            notes: Vec::new(),
        }]);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].anchor.id, "at-vienna-1");
        assert!(hints[0].weight >= 2.0);
        assert_eq!(hints[0].supporting_anchors[0].id, "eu-central-1");
    }
}
