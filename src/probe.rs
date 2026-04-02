use std::net::{IpAddr, ToSocketAddrs};
use std::process::Command;

use anyhow::{Context, Result};

use crate::anchors::Anchor;

#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    pub ping_count: u8,
    pub timeout_ms: u64,
    pub trace_hops: u8,
}

#[derive(Debug, Clone)]
pub struct PingStats {
    pub transmitted: u32,
    pub received: u32,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub mdev_ms: f64,
}

#[derive(Debug, Clone)]
pub struct TraceHop {
    pub hop: u8,
    pub address: Option<String>,
    pub rtt_ms: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub hops: Vec<TraceHop>,
}

#[derive(Debug, Clone)]
pub struct Measurement {
    pub anchor: Anchor,
    pub resolved_ip: Option<IpAddr>,
    pub ping: Option<PingStats>,
    pub trace: Option<TraceSummary>,
    pub notes: Vec<String>,
}

impl PingStats {
    pub fn upper_bound_km(&self, km_per_ms: f64) -> f64 {
        self.min_ms * km_per_ms
    }
}

pub fn probe_anchor(anchor: Anchor, config: &ProbeConfig, include_trace: bool) -> Measurement {
    let mut notes = Vec::new();
    let resolved_ip = resolve_anchor(&anchor).ok().flatten();
    if resolved_ip.is_none() {
        notes.push("DNS resolution failed".to_string());
    }

    let ping_output = run_ping(&anchor, config);
    let ping = match ping_output {
        Ok(output) => {
            let parsed = parse_ping_output(&output.stdout, &output.stderr);
            if parsed.stats.is_none() {
                if !parsed.notes.is_empty() {
                    notes.extend(parsed.notes);
                } else {
                    notes.push("ping returned no usable RTT sample".to_string());
                }
            } else {
                notes.extend(parsed.notes);
            }
            parsed.stats
        }
        Err(error) => {
            notes.push(format!("ping failed: {error:#}"));
            None
        }
    };

    let trace = if include_trace {
        let target = resolved_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| anchor.host.to_string());
        match run_tracepath(&target, config) {
            Ok(output) => {
                let parsed = parse_tracepath_output(&output.stdout, &output.stderr);
                notes.extend(parsed.notes);
                parsed.summary
            }
            Err(error) => {
                notes.push(format!("tracepath failed: {error:#}"));
                None
            }
        }
    } else {
        None
    };

    Measurement {
        anchor,
        resolved_ip,
        ping,
        trace,
        notes,
    }
}

fn resolve_anchor(anchor: &Anchor) -> Result<Option<IpAddr>> {
    let mut addrs = format!("{}:0", anchor.host)
        .to_socket_addrs()
        .with_context(|| format!("resolving {}", anchor.host))?;

    Ok(addrs
        .find(|addr| addr.is_ipv4())
        .map(|addr| addr.ip())
        .or_else(|| addrs.next().map(|addr| addr.ip())))
}

struct CommandOutput {
    stdout: String,
    stderr: String,
}

fn run_ping(anchor: &Anchor, config: &ProbeConfig) -> Result<CommandOutput> {
    let timeout_secs = config.timeout_ms.div_ceil(1000).max(1);
    let output = Command::new("ping")
        .args([
            "-4",
            "-n",
            "-c",
            &config.ping_count.to_string(),
            "-W",
            &timeout_secs.to_string(),
            anchor.host,
        ])
        .output()
        .with_context(|| format!("running ping against {}", anchor.host))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn run_tracepath(target: &str, config: &ProbeConfig) -> Result<CommandOutput> {
    let output = Command::new("tracepath")
        .args(["-n", "-m", &config.trace_hops.to_string(), target])
        .output()
        .with_context(|| format!("running tracepath against {target}"))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[derive(Debug)]
struct ParsedPing {
    stats: Option<PingStats>,
    notes: Vec<String>,
}

fn parse_ping_output(stdout: &str, stderr: &str) -> ParsedPing {
    let mut notes = Vec::new();
    if !stderr.trim().is_empty() {
        notes.push(stderr.trim().to_string());
    }

    let mut transmitted = None;
    let mut received = None;
    let mut stats = None;

    for line in stdout.lines() {
        let trimmed = line.trim();

        if trimmed.contains("packets transmitted") {
            let parts = trimmed.split(',').map(str::trim).collect::<Vec<_>>();
            if let Some(part) = parts.first() {
                transmitted = part
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u32>().ok());
            }
            if let Some(part) = parts.get(1) {
                received = part
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u32>().ok());
            }
        }

        if let Some(values) = trimmed.strip_prefix("rtt min/avg/max/mdev = ") {
            let values = values
                .trim_end_matches(" ms")
                .split('/')
                .collect::<Vec<_>>();
            if values.len() == 4 {
                let min_ms = values[0].parse::<f64>().ok();
                let avg_ms = values[1].parse::<f64>().ok();
                let max_ms = values[2].parse::<f64>().ok();
                let mdev_ms = values[3].parse::<f64>().ok();

                if let (Some(min_ms), Some(avg_ms), Some(max_ms), Some(mdev_ms)) =
                    (min_ms, avg_ms, max_ms, mdev_ms)
                {
                    stats = Some(PingStats {
                        transmitted: transmitted.unwrap_or_default(),
                        received: received.unwrap_or_default(),
                        min_ms,
                        avg_ms,
                        max_ms,
                        mdev_ms,
                    });
                }
            }
        }
    }

    if stats.is_none() && received.unwrap_or_default() == 0 {
        notes.push("no ICMP replies".to_string());
    }

    ParsedPing { stats, notes }
}

#[derive(Debug)]
struct ParsedTrace {
    summary: Option<TraceSummary>,
    notes: Vec<String>,
}

fn parse_tracepath_output(stdout: &str, stderr: &str) -> ParsedTrace {
    let mut notes = Vec::new();
    if !stderr.trim().is_empty() {
        notes.push(stderr.trim().to_string());
    }

    let mut hops = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let Some((hop_token, remainder)) = trimmed.split_once(':') else {
            continue;
        };

        if hop_token.contains('?') {
            continue;
        }

        let hop_token = hop_token.trim();
        let Ok(hop) = hop_token.parse::<u8>() else {
            continue;
        };

        let remainder = remainder.trim();
        if remainder.starts_with("no reply") {
            push_or_merge_hop(
                &mut hops,
                TraceHop {
                    hop,
                    address: None,
                    rtt_ms: None,
                    note: Some("no reply".to_string()),
                },
            );
            continue;
        }

        let tokens = remainder.split_whitespace().collect::<Vec<_>>();
        let address = tokens.first().map(|token| (*token).to_string());
        let rtt_index = tokens.iter().position(|token| token.ends_with("ms"));
        let rtt_ms = rtt_index
            .and_then(|index| tokens.get(index))
            .and_then(|token| token.trim_end_matches("ms").parse::<f64>().ok());
        let note = rtt_index.and_then(|index| {
            if index + 1 < tokens.len() {
                Some(tokens[index + 1..].join(" "))
            } else {
                None
            }
        });

        push_or_merge_hop(
            &mut hops,
            TraceHop {
                hop,
                address,
                rtt_ms,
                note,
            },
        );
    }

    let summary = if hops.is_empty() {
        None
    } else {
        Some(TraceSummary { hops })
    };

    ParsedTrace { summary, notes }
}

fn push_or_merge_hop(hops: &mut Vec<TraceHop>, next: TraceHop) {
    if let Some(existing) = hops.iter_mut().find(|hop| hop.hop == next.hop) {
        if existing.address.is_none() {
            existing.address = next.address;
        }
        existing.rtt_ms = match (existing.rtt_ms, next.rtt_ms) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (None, Some(right)) => Some(right),
            (left, None) => left,
        };
        if existing.note.is_none() || existing.note.as_deref() == Some("no reply") {
            existing.note = next.note;
        }
    } else {
        hops.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ping_stats() {
        let stdout = r#"PING ec2.eu-central-1.amazonaws.com (52.94.137.21) 56(84) bytes of data.
64 bytes from 52.94.137.21: icmp_seq=1 ttl=244 time=1.16 ms
64 bytes from 52.94.137.21: icmp_seq=2 ttl=244 time=1.05 ms

--- ec2.eu-central-1.amazonaws.com ping statistics ---
2 packets transmitted, 2 received, 0% packet loss, time 1001ms
rtt min/avg/max/mdev = 1.051/1.104/1.158/0.053 ms
"#;
        let parsed = parse_ping_output(stdout, "");
        let stats = parsed.stats.expect("stats should parse");

        assert_eq!(stats.transmitted, 2);
        assert_eq!(stats.received, 2);
        assert!((stats.min_ms - 1.051).abs() < 0.0001);
    }

    #[test]
    fn parses_tracepath_hops() {
        let stdout = r#" 1?: [LOCALHOST]                      pmtu 1500
 1:  5.101.109.1                                           1.212ms
 2:  143.244.192.112                                       0.798ms
 3:  143.244.224.110                                       1.434ms asymm  5
 4:  no reply
     Too many hops: pmtu 1500
"#;
        let parsed = parse_tracepath_output(stdout, "");
        let summary = parsed.summary.expect("trace should parse");

        assert_eq!(summary.hops.len(), 4);
        assert_eq!(summary.hops[0].hop, 1);
        assert_eq!(summary.hops[0].address.as_deref(), Some("5.101.109.1"));
        assert_eq!(summary.hops[3].note.as_deref(), Some("no reply"));
    }
}
