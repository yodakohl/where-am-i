use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::probe::{LocalRttFloor, Measurement, PingStats, ProbeMethod};

const CACHE_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq)]
pub struct CachedPing {
    pub method: ProbeMethod,
    pub min_ms: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeCache {
    pub anchor_floors: BTreeMap<String, CachedPing>,
    pub local_rtt_floor_ms: Option<f64>,
    pub tcp_bias_ms: Option<f64>,
}

impl ProbeCache {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Self::parse(&content))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, self.render())
            .with_context(|| format!("writing {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))
    }

    pub fn parse(content: &str) -> Self {
        let mut cache = Self::default();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let parts = trimmed.split_whitespace().collect::<Vec<_>>();
            match parts.as_slice() {
                ["version", CACHE_VERSION] => {}
                ["local_rtt_floor_ms", value] => {
                    cache.local_rtt_floor_ms = value.parse::<f64>().ok();
                }
                ["tcp_bias_ms", value] => {
                    cache.tcp_bias_ms = value.parse::<f64>().ok();
                }
                ["anchor", id, method, value] => {
                    let Some(method) = ProbeMethod::parse(method) else {
                        continue;
                    };
                    let Some(min_ms) = value.parse::<f64>().ok() else {
                        continue;
                    };
                    cache.update_anchor(id, method, min_ms);
                }
                _ => {}
            }
        }

        cache
    }

    pub fn render(&self) -> String {
        let mut lines = vec![format!("version {CACHE_VERSION}")];

        if let Some(local_rtt_floor_ms) = self.local_rtt_floor_ms {
            lines.push(format!("local_rtt_floor_ms {:.6}", local_rtt_floor_ms));
        }
        if let Some(tcp_bias_ms) = self.tcp_bias_ms {
            lines.push(format!("tcp_bias_ms {:.6}", tcp_bias_ms));
        }

        for (anchor_id, ping) in &self.anchor_floors {
            lines.push(format!(
                "anchor {} {} {:.6}",
                anchor_id,
                ping.method.label(),
                ping.min_ms
            ));
        }

        lines.join("\n") + "\n"
    }

    pub fn update_anchor(&mut self, anchor_id: &str, method: ProbeMethod, min_ms: f64) {
        if !min_ms.is_finite() || min_ms <= 0.0 {
            return;
        }

        match self.anchor_floors.get(anchor_id) {
            Some(existing) if existing.min_ms <= min_ms => {}
            _ => {
                self.anchor_floors
                    .insert(anchor_id.to_string(), CachedPing { method, min_ms });
            }
        }
    }

    pub fn update_from_measurements(&mut self, measurements: &[Measurement]) {
        for measurement in measurements {
            let Some(ping) = &measurement.ping else {
                continue;
            };
            self.update_anchor(measurement.anchor.id, ping.method, ping.min_ms);
        }
    }

    pub fn update_local_rtt_floor(&mut self, local_rtt_floor: Option<&LocalRttFloor>) {
        let Some(local_rtt_floor) = local_rtt_floor else {
            return;
        };

        self.local_rtt_floor_ms = match self.local_rtt_floor_ms {
            Some(existing) => Some(existing.min(local_rtt_floor.min_ms)),
            None => Some(local_rtt_floor.min_ms),
        };
    }

    pub fn update_tcp_bias(&mut self, tcp_bias_ms: f64) {
        if !tcp_bias_ms.is_finite() || tcp_bias_ms <= 0.0 {
            return;
        }

        self.tcp_bias_ms = match self.tcp_bias_ms {
            Some(existing) => Some(existing.min(tcp_bias_ms)),
            None => Some(tcp_bias_ms),
        };
    }
}

pub fn cached_ping_to_stats(ping: &CachedPing) -> PingStats {
    PingStats {
        method: ping.method,
        transmitted: 1,
        received: 1,
        min_ms: ping.min_ms,
        avg_ms: ping.min_ms,
        max_ms: ping.min_ms,
        mdev_ms: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_cache() {
        let content = "\
version 1
local_rtt_floor_ms 0.230000
tcp_bias_ms 1.890000
anchor eu-central-1 icmp 0.290000
anchor ca-central-1 tcp/443 90.750000
";
        let cache = ProbeCache::parse(content);

        assert_eq!(cache.local_rtt_floor_ms, Some(0.23));
        assert_eq!(cache.tcp_bias_ms, Some(1.89));
        assert_eq!(
            cache.anchor_floors.get("eu-central-1"),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 0.29,
            })
        );
        assert_eq!(
            cache.anchor_floors.get("ca-central-1"),
            Some(&CachedPing {
                method: ProbeMethod::Tcp { port: 443 },
                min_ms: 90.75,
            })
        );

        let rendered = cache.render();
        assert!(rendered.contains("anchor eu-central-1 icmp 0.290000"));
        assert!(rendered.contains("anchor ca-central-1 tcp/443 90.750000"));
    }

    #[test]
    fn keeps_lower_anchor_floor() {
        let mut cache = ProbeCache::default();
        cache.update_anchor("eu-central-1", ProbeMethod::Icmp, 0.50);
        cache.update_anchor("eu-central-1", ProbeMethod::Tcp { port: 443 }, 0.80);
        cache.update_anchor("eu-central-1", ProbeMethod::Icmp, 0.29);

        assert_eq!(
            cache.anchor_floors.get("eu-central-1"),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 0.29,
            })
        );
    }
}
