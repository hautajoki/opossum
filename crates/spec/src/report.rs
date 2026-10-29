//! Reports: several runs' reports as one, and the policy every target exits by.
//!
//! A report is the runtime's JSON (`runtime/spec.luau`, `RunReport`): `ok`, `mode`, `filter`, `only`, `focused`,
//! `duration_seconds`, `discovered`, `selected`, the `test_*` and `bench_*` counts, and the lists `failures`,
//! `skipped`, `tests` and `benchmarks`. It stays a [`Value`]: the runtime ships in the binary that reads it.

use serde_json::{Value, json};

use crate::Mode;

/// The counts a report sums.
const COUNTS: [&str; 11] = [
    "discovered",
    "selected",
    "only",
    "test_total",
    "test_pass",
    "test_fail",
    "test_skip",
    "bench_total",
    "bench_pass",
    "bench_fail",
    "bench_skip",
];

/// The lists a report concatenates.
const LISTS: [&str; 4] = ["failures", "skipped", "tests", "benchmarks"];

/// One report of `reports`, in their order: counts and durations summed, `ok` and-ed, `focused` or-ed, lists
/// concatenated. `mode` and `filter` are the run's, so no reports at all still make a report.
pub fn aggregate(reports: &[Value], mode: Mode, filter: Option<&str>) -> Value {
    let mut out = json!({
        "ok": reports.iter().all(|r| r["ok"] == true),
        "mode": mode.name(),
        "focused": reports.iter().any(|r| r["focused"] == true),
        "duration_seconds": reports.iter().filter_map(|r| r["duration_seconds"].as_f64()).sum::<f64>(),
    });
    if let Some(filter) = filter {
        out["filter"] = filter.into();
    }
    for key in COUNTS {
        out[key] = reports.iter().filter_map(|r| r[key].as_u64()).sum::<u64>().into();
    }
    for key in LISTS {
        let list: Vec<Value> = reports.iter().flat_map(|r| r[key].as_array().into_iter().flatten().cloned()).collect();
        out[key] = list.into();
    }
    out
}

/// Why a run that ran exits 1, as a diagnostic code and message: it selected nothing, it was focused without
/// `allow_focused` (a forgotten `.only` must not narrow a release run), or a case failed. `None` passes.
pub fn policy(report: &Value, allow_focused: bool) -> Option<(&'static str, String)> {
    if report["selected"].as_u64() == Some(0) {
        return Some(("test.noneSelected", none_selected(report)));
    }
    if report["focused"] == true && !allow_focused {
        let message = "focused cases are not allowed; remove `.only` or pass --allow-focused for an intentional run";
        return Some(("test.focused", message.to_owned()));
    }
    if report["ok"] != true {
        return Some(("test.failed", "one or more cases failed".to_owned()));
    }
    None
}

/// `no cases matched “filter” (n discovered)`, the filter only when there is one.
pub(crate) fn none_selected(report: &Value) -> String {
    let filter = report["filter"].as_str().map_or(String::new(), |filter| format!(" “{filter}”"));
    format!("no cases matched{filter} ({} discovered)", report["discovered"].as_u64().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(discovered: u64, selected: u64, pass: u64) -> Value {
        let mut report = aggregate(&[], Mode::Test, None);
        report["discovered"] = discovered.into();
        report["selected"] = selected.into();
        report["test_total"] = pass.into();
        report["test_pass"] = pass.into();
        report
    }

    #[test]
    fn empty_focused_and_failed_runs_exit_1() {
        let empty = report(3, 0, 0);
        assert_eq!(policy(&empty, true), Some(("test.noneSelected", "no cases matched (3 discovered)".to_owned())));
        let mut focused = report(3, 1, 1);
        focused["focused"] = true.into();
        assert_eq!(policy(&focused, false).map(|p| p.0), Some("test.focused"));
        assert_eq!(policy(&focused, true), None);
        focused["ok"] = false.into();
        assert_eq!(policy(&focused, true).map(|p| p.0), Some("test.failed"));
    }

    #[test]
    fn aggregates_keep_every_file_selection() {
        let total = aggregate(&[report(3, 0, 0), report(2, 2, 2)], Mode::Test, Some("voxel"));
        let counts = |key: &str| total[key].as_u64();
        assert_eq!((counts("discovered"), counts("selected"), counts("test_pass")), (Some(5), Some(2), Some(2)));
        assert_eq!(total["filter"], "voxel");
        assert_eq!(policy(&total, false), None);
        let none = aggregate(&[], Mode::Bench, Some("x"));
        assert_eq!(policy(&none, false), Some(("test.noneSelected", "no cases matched “x” (0 discovered)".to_owned())));
    }
}
