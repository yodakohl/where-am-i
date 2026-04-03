use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::anchors::Anchor;

const MAX_RESOLVED_IPS_PER_ROUND: usize = 3;

#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    pub ping_count: u8,
    pub timeout_ms: u64,
    pub trace_hops: u8,
    pub rounds: u8,
}

#[derive(Debug, Clone)]
pub struct PingStats {
    pub method: ProbeMethod,
    pub transmitted: u32,
    pub received: u32,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub mdev_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMethod {
    Icmp,
    Tcp { port: u16 },
}

#[derive(Debug, Clone)]
pub struct TraceHop {
    pub hop: u8,
    pub address: Option<String>,
    pub hostname: Option<String>,
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

#[derive(Debug, Clone)]
pub struct LocalRttFloor {
    pub target: String,
    pub source: String,
    pub min_ms: f64,
}

impl PingStats {
    pub fn upper_bound_km(&self, km_per_ms: f64) -> f64 {
        self.min_ms * km_per_ms
    }
}

impl ProbeMethod {
    pub fn label(&self) -> String {
        match self {
            Self::Icmp => "icmp".to_string(),
            Self::Tcp { port } => format!("tcp/{port}"),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value == "icmp" {
            return Some(Self::Icmp);
        }

        let port = value.strip_prefix("tcp/")?.parse::<u16>().ok()?;
        Some(Self::Tcp { port })
    }
}

pub fn measure_local_rtt_floor(config: &ProbeConfig) -> Option<LocalRttFloor> {
    let mut candidates = Vec::new();

    if let Some(gateway_ip) = default_gateway_ip() {
        candidates.push((gateway_ip, "default gateway".to_string()));
    }

    candidates.extend(tracepath_local_candidates(config));

    let local_config = ProbeConfig {
        ping_count: config.ping_count.max(5),
        ..*config
    };
    let local_attempts = usize::from(config.rounds.max(2));

    let mut best: Option<LocalRttFloor> = None;

    for (target, source) in candidates {
        let mut candidate_min_ms: Option<f64> = None;

        for _ in 0..local_attempts {
            let Ok(output) = run_ping(&target, &local_config) else {
                continue;
            };
            let parsed = parse_ping_output(&output.stdout, &output.stderr);
            let Some(stats) = parsed.stats else {
                continue;
            };

            candidate_min_ms = match candidate_min_ms {
                Some(current) => Some(current.min(stats.min_ms)),
                None => Some(stats.min_ms),
            };
        }

        let Some(min_ms) = candidate_min_ms else {
            continue;
        };

        let next = LocalRttFloor {
            target,
            source,
            min_ms,
        };

        match &best {
            Some(current) if current.min_ms <= next.min_ms => {}
            _ => best = Some(next),
        }
    }

    best
}

pub fn measure_tcp_handshake(target: &str, timeout_ms: u64) -> Option<PingStats> {
    let config = ProbeConfig {
        ping_count: 1,
        timeout_ms,
        trace_hops: 1,
        rounds: 1,
    };

    run_tcp_probe(target, &config).ok().flatten()
}

pub fn probe_anchor(anchor: Anchor, config: &ProbeConfig, include_trace: bool) -> Measurement {
    let mut notes = Vec::new();
    let mut resolved_ip = None;
    let mut ping = None;
    let mut successful_rounds = 0u8;
    let rounds = config.rounds.max(1);

    for round in 1..=rounds {
        let round_ips = resolve_anchor_ips(&anchor).ok().unwrap_or_default();
        let address_count = round_ips.len().min(MAX_RESOLVED_IPS_PER_ROUND);
        if round_ips.is_empty() {
            notes.push(format!("round {round}: DNS resolution failed"));
        }

        let targets = if round_ips.is_empty() {
            vec![(None, anchor.host.to_string())]
        } else {
            round_ips
                .into_iter()
                .take(MAX_RESOLVED_IPS_PER_ROUND)
                .map(|ip| (Some(ip), ip.to_string()))
                .collect::<Vec<_>>()
        };

        let mut round_best = None;
        let mut round_failures = 0usize;
        let mut round_notes = Vec::new();

        for (candidate_ip, target) in targets {
            let (sample, sample_notes) = measure_target(&target, config);
            if let Some(stats) = sample {
                let replace = round_best
                    .as_ref()
                    .map(|(_, best): &(Option<IpAddr>, PingStats)| stats.min_ms < best.min_ms)
                    .unwrap_or(true);
                if replace {
                    round_best = Some((candidate_ip, stats));
                }
            } else {
                round_failures += 1;
                round_notes.extend(
                    sample_notes
                        .into_iter()
                        .take(2)
                        .map(|note| format!("round {round}: {target}: {note}")),
                );
            }
        }

        if let Some((round_ip, stats)) = round_best {
            successful_rounds += 1;
            let replace = ping
                .as_ref()
                .map(|best: &PingStats| stats.min_ms < best.min_ms)
                .unwrap_or(true);
            if replace {
                resolved_ip = round_ip;
                ping = Some(stats);
            }
        } else if round_failures > 0 {
            notes.push(format!(
                "round {round}: no usable RTT sample across {round_failures} target(s)"
            ));
            notes.extend(round_notes);
        }

        if address_count > 1 {
            notes.push(format!(
                "round {round}: selected best RTT from {address_count} resolved IPs"
            ));
        }
    }

    if ping.is_none() && rounds == 1 && notes.is_empty() {
        notes.push("ping returned no usable RTT sample".to_string());
    }
    if rounds > 1 && successful_rounds != rounds {
        notes.push(format!(
            "successful probe rounds: {successful_rounds}/{rounds}"
        ));
    }

    let trace = if include_trace {
        let target = resolved_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| anchor.host.to_string());
        match run_tracepath(&target, config) {
            Ok(output) => {
                let parsed = parse_tracepath_output(&output.stdout, &output.stderr);
                notes.extend(parsed.notes);
                parsed.summary.map(enrich_trace_hostnames)
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

fn resolve_anchor_ips(anchor: &Anchor) -> Result<Vec<IpAddr>> {
    let addrs = format!("{}:0", anchor.host)
        .to_socket_addrs()
        .with_context(|| format!("resolving {}", anchor.host))?;

    let mut ips = Vec::new();
    for addr in addrs {
        let ip = addr.ip();
        if ip.is_ipv4() && !ips.contains(&ip) {
            ips.push(ip);
        }
    }

    if ips.is_empty() {
        let fallback = format!("{}:0", anchor.host)
            .to_socket_addrs()
            .with_context(|| format!("resolving {}", anchor.host))?;
        for addr in fallback {
            let ip = addr.ip();
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
    }

    Ok(ips)
}

fn default_gateway_ip() -> Option<String> {
    let output = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let tokens = line.split_whitespace().collect::<Vec<_>>();
            let via_index = tokens.iter().position(|token| *token == "via")?;
            tokens.get(via_index + 1).map(|ip| (*ip).to_string())
        })
}

fn tracepath_local_candidates(config: &ProbeConfig) -> Vec<(String, String)> {
    let trace_config = ProbeConfig {
        trace_hops: config.trace_hops.min(3).max(2),
        ..*config
    };
    let Ok(output) = run_tracepath("1.1.1.1", &trace_config) else {
        return Vec::new();
    };
    let parsed = parse_tracepath_output(&output.stdout, &output.stderr);
    let Some(summary) = parsed.summary else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for hop in summary.hops.into_iter().take(2) {
        if let Some(address) = hop.address {
            if !candidates.iter().any(|(existing, _)| existing == &address) {
                candidates.push((address, format!("tracepath hop {}", hop.hop)));
            }
        }
    }

    candidates
}

fn measure_target(target: &str, config: &ProbeConfig) -> (Option<PingStats>, Vec<String>) {
    let mut notes = Vec::new();

    match run_ping(target, config) {
        Ok(output) => {
            let parsed = parse_ping_output(&output.stdout, &output.stderr);
            if let Some(stats) = parsed.stats {
                return (Some(stats), parsed.notes);
            }
            notes.extend(parsed.notes);
        }
        Err(error) => {
            notes.push(format!("ping failed: {error:#}"));
        }
    }

    match run_tcp_probe(target, config) {
        Ok(Some(stats)) => {
            notes.push(format!("used {} fallback", stats.method.label()));
            (Some(stats), notes)
        }
        Ok(None) => (None, notes),
        Err(error) => {
            notes.push(format!("tcp probe failed: {error:#}"));
            (None, notes)
        }
    }
}

struct CommandOutput {
    stdout: String,
    stderr: String,
}

fn run_ping(target: &str, config: &ProbeConfig) -> Result<CommandOutput> {
    let timeout_secs = config.timeout_ms.div_ceil(1000).max(1);
    let output = Command::new("ping")
        .args([
            "-4",
            "-n",
            "-c",
            &config.ping_count.to_string(),
            "-W",
            &timeout_secs.to_string(),
            target,
        ])
        .output()
        .with_context(|| format!("running ping against {target}"))?;

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

fn run_tcp_probe(target: &str, config: &ProbeConfig) -> Result<Option<PingStats>> {
    for port in [443_u16, 80_u16] {
        let addrs = format!("{target}:{port}")
            .to_socket_addrs()
            .with_context(|| format!("resolving TCP target {target}:{port}"))?
            .collect::<Vec<_>>();

        for addr in addrs {
            let start = Instant::now();
            if TcpStream::connect_timeout(&addr, Duration::from_millis(config.timeout_ms)).is_ok() {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                return Ok(Some(PingStats {
                    method: ProbeMethod::Tcp { port },
                    transmitted: 1,
                    received: 1,
                    min_ms: elapsed_ms,
                    avg_ms: elapsed_ms,
                    max_ms: elapsed_ms,
                    mdev_ms: 0.0,
                }));
            }
        }
    }

    Ok(None)
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
                        method: ProbeMethod::Icmp,
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
                    hostname: None,
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
                hostname: None,
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

fn enrich_trace_hostnames(mut summary: TraceSummary) -> TraceSummary {
    for hop in &mut summary.hops {
        hop.hostname = hop
            .address
            .as_deref()
            .and_then(reverse_lookup_ip)
            .filter(|hostname| hostname != hop.address.as_deref().unwrap_or_default());
    }
    summary
}

fn push_or_merge_hop(hops: &mut Vec<TraceHop>, next: TraceHop) {
    if let Some(existing) = hops.iter_mut().find(|hop| hop.hop == next.hop) {
        if existing.address.is_none() {
            existing.address = next.address;
        }
        if existing.hostname.is_none() {
            existing.hostname = next.hostname;
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

fn reverse_lookup_ip(ip: &str) -> Option<String> {
    let output = Command::new("getent").args(["hosts", ip]).output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let first = parts.next()?;
            let second = parts.next()?;
            if first == ip {
                Some(second.to_string())
            } else {
                None
            }
        })
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
        assert_eq!(stats.method, ProbeMethod::Icmp);
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
        assert!(summary.hops[0].hostname.is_none());
        assert_eq!(summary.hops[3].note.as_deref(), Some("no reply"));
    }
}
