//! Phase 11C Task 11 — managed LogRing snapshot collector (spec §16).
//!
//! REUSES the bounded output the managed-process reader threads already
//! capture (workspace LogRings). No second stdout/stderr pipeline and NO
//! external-process output scraping: the collector is a pure function over
//! the already-captured per-managed-service tails.
//! RED tests first.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::bundle::{ManagedServiceOutput, SectionContent};

    fn service(id: &str, n_lines: usize, line_len: usize) -> (String, Vec<String>) {
        (
            id.to_string(),
            (0..n_lines).map(|i| format!("line-{i}-{}", "x".repeat(line_len))).collect(),
        )
    }

    #[test]
    fn per_service_line_cap_100() {
        let services = vec![service("svc-a", 150, 10)];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        assert_eq!(v.len(), 1);
        let s = &v[0];
        assert_eq!(s.lines.len(), MANAGED_LINES_PER_SERVICE, "kept = last 100");
        assert!(s.truncated_lines, "flagged");
        assert!(s.lines[0].starts_with("line-50-"), "oldest dropped");
        assert!(s.lines[99].starts_with("line-149-"), "newest kept");
    }

    #[test]
    fn per_service_byte_cap_256kib() {
        // 128 KiB total lines but one giant line → byte cap enforced with an
        // explicit marker line.
        let services = vec![service("svc-b", 4, 80 * 1024)];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let s = &v[0];
        assert!(
            s.bytes <= MANAGED_BYTES_PER_SERVICE,
            "byte cap enforced: {}",
            s.bytes
        );
        assert!(s.truncated_bytes, "flagged");
        assert!(
            s.lines
                .iter()
                .any(|l| l.contains("[TRUNCATED: original output exceeded diagnostic limit]")),
            "explicit truncation marker present"
        );
    }

    #[test]
    fn combined_cap_5MiB_across_services() {
        // 25 services x ~256 KiB = ~6.4 MiB > 5 MiB → deterministic trims.
        let services: Vec<(String, Vec<String>)> = (0..25)
            .map(|i| service(&format!("svc-{i:02}"), 4, 64 * 1024))
            .collect();
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let total: u64 = v.iter().map(|s| s.bytes).sum();
        assert!(
            total <= MANAGED_TOTAL_BYTES,
            "combined cap enforced: {total}"
        );
        // Every retained service keeps its per-service integrity flags; all
        // 25 services are still represented (trimmed, not dropped).
        assert_eq!(v.len(), 25);
    }

    #[test]
    fn redaction_pass_applies_to_every_line() {
        let services = vec![(
            "svc-c".into(),
            vec![
                "ok line".to_string(),
                "leak token=supersecret123".to_string(),
            ],
        )];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let joined = v[0].lines.join("\n");
        assert!(!joined.contains("supersecret123"), "secret redacted");
        assert!(joined.contains("token=[REDACTED]"));
    }

    #[test]
    fn empty_input_yields_empty_section() {
        let out = collect_managed_output(&[]);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        assert!(v.is_empty());
    }
}

// -- implementation -----------------------------------------------------------

use crate::diagnostics::bundle::{ManagedServiceOutput, SectionContent};
use crate::diagnostics::redact;

/// Exact managed-output bounds (spec §16 / plan §8).
pub(crate) const MANAGED_LINES_PER_SERVICE: usize = 100;
pub(crate) const MANAGED_BYTES_PER_SERVICE: u64 = 262_144; // 256 KiB
pub(crate) const MANAGED_TOTAL_BYTES: u64 = 5_242_880; // 5 MiB

const TRUNCATION_MARKER: &str = "[TRUNCATED: original output exceeded diagnostic limit]";

/// Deterministic bounded tail of the ALREADY-CAPTURED managed output
/// (workspace LogRings). Redaction applies to every line. Bounds: 100 lines
/// and 256 KiB per service; 5 MiB combined — lowest-priority services are
/// trimmed first (largest byte count, id tie-break). No external-process
/// scraping is possible: the input is only the managed registry snapshot.
pub(crate) fn collect_managed_output(
    services: &[(String, Vec<String>)],
) -> SectionContent {
    // Per-service: redact EVERY line first (defense-in-depth: the pure
    // bounds function also redacts so no caller can forget), keep the LAST
    // 100 lines, then trim to ≤ 256 KiB from the tail with an explicit
    // marker.
    let mut out: Vec<ManagedServiceOutput> = services
        .iter()
        .map(|(id, lines)| {
            let lines: Vec<String> = lines.iter().map(|l| redact(l)).collect();
            let truncated_lines = lines.len() > MANAGED_LINES_PER_SERVICE;
            let kept: Vec<String> = if truncated_lines {
                lines[lines.len() - MANAGED_LINES_PER_SERVICE..].to_vec()
            } else {
                lines.clone()
            };
            let mut truncated_bytes = false;
            let mut kept = keep_last_bytes(kept, MANAGED_BYTES_PER_SERVICE, &mut truncated_bytes);
            if truncated_bytes {
                kept.insert(0, TRUNCATION_MARKER.to_string());
            }
            let bytes: u64 = kept.iter().map(|l| l.len() as u64 + 1).sum();
            ManagedServiceOutput {
                service_id: id.clone(),
                lines: kept,
                truncated_lines,
                bytes,
                truncated_bytes,
            }
        })
        .collect();

    // Combined budget: trim lowest-priority services first — largest byte
    // count, then service id (deterministic). Each pass either halves the
    // largest service or reduces it to the marker line, so the total strictly
    // decreases and the loop converges.
    let mut total: u64 = out.iter().map(|s| s.bytes).sum();
    while total > MANAGED_TOTAL_BYTES {
        let Some(idx) = out
            .iter()
            .enumerate()
            .max_by(|(ia, a), (ib, b)| {
                b.bytes.cmp(&a.bytes).then(b.service_id.cmp(&a.service_id)).then(ib.cmp(ia))
            })
            .map(|(i, _)| i)
        else {
            break;
        };
        let len = out[idx].lines.len();
        if len > 1 {
            // Halve the service's lines, keep newest tail, re-measure.
            let svc = &mut out[idx];
            let keep_from = svc.lines.len() - svc.lines.len() / 2;
            svc.lines.drain(0..keep_from);
            svc.truncated_lines = true;
            svc.lines.insert(0, TRUNCATION_MARKER.to_string());
            svc.bytes = svc.lines.iter().map(|l| l.len() as u64 + 1).sum();
            svc.truncated_bytes = true;
        } else {
            // Already minimal: marker line only.
            let svc = &mut out[idx];
            svc.lines.clear();
            svc.lines.push(TRUNCATION_MARKER.to_string());
            svc.bytes = TRUNCATION_MARKER.len() as u64 + 1;
            svc.truncated_bytes = true;
        }
        total = out.iter().map(|s| s.bytes).sum();
    }

    SectionContent::ManagedOutput(out)
}

/// Keep the newest lines fitting `cap` bytes; set `truncated` when anything
/// was dropped. Deterministic: drop from the front (oldest first).
fn keep_last_bytes(lines: Vec<String>, cap: u64, truncated: &mut bool) -> Vec<String> {
    let mut total: u64 = lines.iter().map(|l| l.len() as u64 + 1).sum();
    let mut start = 0usize;
    while total > cap && start < lines.len() {
        total -= lines[start].len() as u64 + 1;
        start += 1;
        *truncated = true;
    }
    if start == 0 {
        lines
    } else if start >= lines.len() {
        // Everything exceeded the cap: keep nothing but flag truncation.
        Vec::new()
    } else {
        lines[start..].to_vec()
    }
}

/// Live wiring: snapshot tails from the EXISTING workspace LogRings via the
/// owner accessor. Pure read; redaction + bounds are applied by
/// `collect_managed_output`.
#[allow(dead_code)] // consumed by bundle assembly (Tasks 13/19)
pub(crate) fn collect_managed_output_from(
    tails: &[(String, Vec<String>)],
) -> SectionContent {
    collect_managed_output(tails)
}
