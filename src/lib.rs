//! Physics-first network geolocation for Ethernet-connected hosts.
//!
//! `etherwhere` probes a built-in global anchor set with ICMP and TCP fallback,
//! extracts corridor hints from traceroute hostnames, and reports a posterior
//! center plus uncertainty instead of pretending active latency alone can yield
//! exact coordinates everywhere.

pub mod anchors;
pub mod cache;
pub mod hints;
pub mod probe;
pub mod solver;
