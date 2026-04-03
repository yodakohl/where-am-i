use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::probe::{LocalRttFloor, Measurement, PingStats, ProbeMethod};

const CACHE_VERSION: &str = "3";
const LEGACY_PROFILE_ID: &str = "legacy-global";

#[derive(Debug, Clone, PartialEq)]
pub struct CachedPing {
    pub method: ProbeMethod,
    pub min_ms: f64,
    pub observed_day: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedScalar {
    pub min_value: f64,
    pub observed_day: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheProfile {
    pub anchor_floors: BTreeMap<String, CachedPing>,
    pub local_rtt_floor: Option<CachedScalar>,
    pub tcp_bias: Option<CachedScalar>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeCache {
    pub profiles: BTreeMap<String, CacheProfile>,
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
        let mut version = None::<&str>;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let parts = trimmed.split_whitespace().collect::<Vec<_>>();
            match parts.as_slice() {
                ["version", value] => {
                    version = Some(*value);
                }
                ["local_rtt_floor_ms", value] => {
                    cache.update_local_rtt_floor_ms(
                        LEGACY_PROFILE_ID,
                        value.parse::<f64>().ok(),
                        None,
                    );
                }
                ["tcp_bias_ms", value] => {
                    cache.update_tcp_bias_ms(LEGACY_PROFILE_ID, value.parse::<f64>().ok(), None);
                }
                ["anchor", id, method, value] if version.is_none() || version == Some("1") => {
                    let Some(method) = ProbeMethod::parse(method) else {
                        continue;
                    };
                    let Some(min_ms) = value.parse::<f64>().ok() else {
                        continue;
                    };
                    cache.update_anchor(LEGACY_PROFILE_ID, id, method, min_ms, None);
                }
                ["profile", fingerprint, "local_rtt_floor_ms", value]
                    if version == Some(CACHE_VERSION) =>
                {
                    cache.update_local_rtt_floor_ms(fingerprint, value.parse::<f64>().ok(), None);
                }
                ["profile", fingerprint, "tcp_bias_ms", value]
                    if version == Some(CACHE_VERSION) =>
                {
                    cache.update_tcp_bias_ms(fingerprint, value.parse::<f64>().ok(), None);
                }
                ["anchor", fingerprint, id, method, value] if version == Some(CACHE_VERSION) => {
                    let Some(method) = ProbeMethod::parse(method) else {
                        continue;
                    };
                    let Some(min_ms) = value.parse::<f64>().ok() else {
                        continue;
                    };
                    cache.update_anchor(fingerprint, id, method, min_ms, None);
                }
                [
                    "profile",
                    fingerprint,
                    "local_rtt_floor_ms",
                    value,
                    observed_day,
                ] if version == Some(CACHE_VERSION) => {
                    cache.update_local_rtt_floor_ms(
                        fingerprint,
                        value.parse::<f64>().ok(),
                        parse_observed_day(observed_day),
                    );
                }
                ["profile", fingerprint, "tcp_bias_ms", value, observed_day]
                    if version == Some(CACHE_VERSION) =>
                {
                    cache.update_tcp_bias_ms(
                        fingerprint,
                        value.parse::<f64>().ok(),
                        parse_observed_day(observed_day),
                    );
                }
                ["anchor", fingerprint, id, method, value, observed_day]
                    if version == Some(CACHE_VERSION) =>
                {
                    let Some(method) = ProbeMethod::parse(method) else {
                        continue;
                    };
                    let Some(min_ms) = value.parse::<f64>().ok() else {
                        continue;
                    };
                    cache.update_anchor(
                        fingerprint,
                        id,
                        method,
                        min_ms,
                        parse_observed_day(observed_day),
                    );
                }
                _ => {}
            }
        }

        cache
    }

    pub fn render(&self) -> String {
        let mut lines = vec![format!("version {CACHE_VERSION}")];

        for (fingerprint, profile) in &self.profiles {
            if let Some(local_rtt_floor) = &profile.local_rtt_floor {
                lines.push(format!(
                    "profile {} local_rtt_floor_ms {:.6} {}",
                    fingerprint,
                    local_rtt_floor.min_value,
                    render_observed_day(local_rtt_floor.observed_day),
                ));
            }
            if let Some(tcp_bias) = &profile.tcp_bias {
                lines.push(format!(
                    "profile {} tcp_bias_ms {:.6} {}",
                    fingerprint,
                    tcp_bias.min_value,
                    render_observed_day(tcp_bias.observed_day),
                ));
            }

            for (anchor_id, ping) in &profile.anchor_floors {
                lines.push(format!(
                    "anchor {} {} {} {:.6} {}",
                    fingerprint,
                    anchor_id,
                    ping.method.label(),
                    ping.min_ms,
                    render_observed_day(ping.observed_day),
                ));
            }
        }

        lines.join("\n") + "\n"
    }

    pub fn profile(&self, fingerprint: &str) -> Option<&CacheProfile> {
        self.profiles.get(fingerprint)
    }

    pub fn update_anchor(
        &mut self,
        fingerprint: &str,
        anchor_id: &str,
        method: ProbeMethod,
        min_ms: f64,
        observed_day: Option<i64>,
    ) {
        if !min_ms.is_finite() || min_ms <= 0.0 {
            return;
        }

        let profile = self.profile_mut(fingerprint);
        match profile.anchor_floors.get(anchor_id) {
            Some(existing) if existing.min_ms <= min_ms => {}
            _ => {
                profile.anchor_floors.insert(
                    anchor_id.to_string(),
                    CachedPing {
                        method,
                        min_ms,
                        observed_day,
                    },
                );
            }
        }
    }

    pub fn update_from_measurements(&mut self, fingerprint: &str, measurements: &[Measurement]) {
        for measurement in measurements {
            let Some(ping) = &measurement.ping else {
                continue;
            };
            self.update_anchor(
                fingerprint,
                measurement.anchor.id,
                ping.method,
                ping.min_ms,
                current_day_number(),
            );
        }
    }

    pub fn update_local_rtt_floor(
        &mut self,
        fingerprint: &str,
        local_rtt_floor: Option<&LocalRttFloor>,
    ) {
        let Some(local_rtt_floor) = local_rtt_floor else {
            return;
        };

        self.update_local_rtt_floor_ms(
            fingerprint,
            Some(local_rtt_floor.min_ms),
            current_day_number(),
        );
    }

    pub fn update_tcp_bias(&mut self, fingerprint: &str, tcp_bias_ms: f64) {
        self.update_tcp_bias_ms(fingerprint, Some(tcp_bias_ms), current_day_number());
    }

    fn update_local_rtt_floor_ms(
        &mut self,
        fingerprint: &str,
        min_ms: Option<f64>,
        observed_day: Option<i64>,
    ) {
        let Some(min_ms) = min_ms else {
            return;
        };
        if !min_ms.is_finite() || min_ms <= 0.0 {
            return;
        }

        let profile = self.profile_mut(fingerprint);
        profile.local_rtt_floor = match profile.local_rtt_floor.take() {
            Some(existing) if existing.min_value <= min_ms => Some(existing),
            _ => Some(CachedScalar {
                min_value: min_ms,
                observed_day,
            }),
        };
    }

    fn update_tcp_bias_ms(
        &mut self,
        fingerprint: &str,
        tcp_bias_ms: Option<f64>,
        observed_day: Option<i64>,
    ) {
        let Some(tcp_bias_ms) = tcp_bias_ms else {
            return;
        };
        if !tcp_bias_ms.is_finite() || tcp_bias_ms <= 0.0 {
            return;
        }

        let profile = self.profile_mut(fingerprint);
        profile.tcp_bias = match profile.tcp_bias.take() {
            Some(existing) if existing.min_value <= tcp_bias_ms => Some(existing),
            _ => Some(CachedScalar {
                min_value: tcp_bias_ms,
                observed_day,
            }),
        };
    }

    fn profile_mut(&mut self, fingerprint: &str) -> &mut CacheProfile {
        self.profiles.entry(fingerprint.to_string()).or_default()
    }
}

impl CacheProfile {
    pub fn decayed_local_rtt_floor_ms(&self, today_day: Option<i64>) -> Option<f64> {
        Some(
            self.local_rtt_floor.as_ref()?.min_value
                + cache_decay_ms(cache_age_days(
                    self.local_rtt_floor.as_ref()?.observed_day,
                    today_day,
                )),
        )
    }

    pub fn decayed_tcp_bias_ms(&self, today_day: Option<i64>) -> Option<f64> {
        Some(
            self.tcp_bias.as_ref()?.min_value
                + cache_decay_ms(cache_age_days(
                    self.tcp_bias.as_ref()?.observed_day,
                    today_day,
                )),
        )
    }
}

pub fn current_day_number() -> Option<i64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some((now.as_secs() / 86_400) as i64)
}

pub fn cache_age_days(observed_day: Option<i64>, today_day: Option<i64>) -> Option<i64> {
    match (observed_day, today_day) {
        (Some(observed_day), Some(today_day)) => Some((today_day - observed_day).max(0)),
        _ => None,
    }
}

pub fn cache_decay_ms(age_days: Option<i64>) -> f64 {
    match age_days {
        Some(age_days) => (0.25 + age_days as f64 * 0.20).min(6.0),
        None => 3.0,
    }
}

fn parse_observed_day(value: &str) -> Option<i64> {
    let observed_day = value.parse::<i64>().ok()?;
    (observed_day >= 0).then_some(observed_day)
}

fn render_observed_day(observed_day: Option<i64>) -> i64 {
    observed_day.unwrap_or(-1)
}

pub fn cached_ping_to_stats(ping: &CachedPing, effective_min_ms: f64) -> PingStats {
    PingStats {
        method: ping.method,
        transmitted: 1,
        received: 1,
        min_ms: effective_min_ms,
        avg_ms: effective_min_ms,
        max_ms: effective_min_ms,
        mdev_ms: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_versioned_profiles() {
        let content = "\
version 3
profile home local_rtt_floor_ms 0.230000 20000
profile home tcp_bias_ms 1.890000 20000
anchor home eu-central-1 icmp 0.290000 20000
anchor roam ca-central-1 tcp/443 90.750000 20000
";
        let cache = ProbeCache::parse(content);

        let home = cache.profile("home").expect("home profile should exist");
        assert_eq!(
            home.local_rtt_floor.as_ref().map(|value| value.min_value),
            Some(0.23)
        );
        assert_eq!(
            home.tcp_bias.as_ref().map(|value| value.min_value),
            Some(1.89)
        );
        assert_eq!(
            home.anchor_floors.get("eu-central-1"),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 0.29,
                observed_day: Some(20000),
            })
        );
        assert_eq!(
            cache
                .profile("roam")
                .and_then(|profile| profile.anchor_floors.get("ca-central-1")),
            Some(&CachedPing {
                method: ProbeMethod::Tcp { port: 443 },
                min_ms: 90.75,
                observed_day: Some(20000),
            })
        );

        let rendered = cache.render();
        assert!(rendered.contains("profile home local_rtt_floor_ms 0.230000 20000"));
        assert!(rendered.contains("anchor home eu-central-1 icmp 0.290000 20000"));
        assert!(rendered.contains("anchor roam ca-central-1 tcp/443 90.750000 20000"));
    }

    #[test]
    fn parses_legacy_cache_into_legacy_profile() {
        let content = "\
version 1
local_rtt_floor_ms 0.230000
tcp_bias_ms 1.890000
anchor eu-central-1 icmp 0.290000
";
        let cache = ProbeCache::parse(content);
        let profile = cache
            .profile(LEGACY_PROFILE_ID)
            .expect("legacy profile should exist");

        assert_eq!(
            profile
                .local_rtt_floor
                .as_ref()
                .map(|value| value.min_value),
            Some(0.23)
        );
        assert_eq!(
            profile.tcp_bias.as_ref().map(|value| value.min_value),
            Some(1.89)
        );
        assert_eq!(
            profile.anchor_floors.get("eu-central-1"),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 0.29,
                observed_day: None,
            })
        );
    }

    #[test]
    fn keeps_lower_anchor_floor_per_profile() {
        let mut cache = ProbeCache::default();
        cache.update_anchor("home", "eu-central-1", ProbeMethod::Icmp, 0.50, None);
        cache.update_anchor(
            "home",
            "eu-central-1",
            ProbeMethod::Tcp { port: 443 },
            0.80,
            None,
        );
        cache.update_anchor("home", "eu-central-1", ProbeMethod::Icmp, 0.29, None);
        cache.update_anchor("roam", "eu-central-1", ProbeMethod::Icmp, 5.00, None);

        assert_eq!(
            cache
                .profile("home")
                .and_then(|profile| profile.anchor_floors.get("eu-central-1")),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 0.29,
                observed_day: None,
            })
        );
        assert_eq!(
            cache
                .profile("roam")
                .and_then(|profile| profile.anchor_floors.get("eu-central-1")),
            Some(&CachedPing {
                method: ProbeMethod::Icmp,
                min_ms: 5.0,
                observed_day: None,
            })
        );
    }
}
