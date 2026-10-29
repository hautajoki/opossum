//! The one reporter of every target: failures, skips, the slowest tests, benchmarks, then both tallies and where the
//! run ran. It prints a report as it is; mapping its places to project files is done before (`--json` shows the same
//! mapped report).

use serde_json::Value;

/// The text of `report`. `ran` says where it ran (`local`, `edit, generation 12`, `development/main v57`);
/// `project` keeps the places and trace lines that are project frames; `color` paints with ANSI codes.
pub fn print(report: &Value, ran: &str, project: &dyn Fn(&str) -> bool, color: bool) -> String {
    let paint = |code: &str, text: &str| if color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_owned() };
    let list = |key: &str| report[key].as_array().map_or(&[][..], Vec::as_slice);
    let mut out = String::new();

    if !list("failures").is_empty() {
        out += &format!("\n{}\n", paint("1;31", &format!("Failures ({})", list("failures").len())));
        for failure in list("failures") {
            let kind = paint("2", &format!("[{}]", failure["kind"].as_str().unwrap_or("test")));
            out += &format!("  {} {} {kind}\n", paint("31", "✗"), labels(failure));
            for line in failure["message"].as_str().unwrap_or("").lines() {
                out += &format!("      {line}\n");
            }
            // the place a case failed at, when it is the project's: a suite Studio does not hold fails in the host
            if let (Some(source), Some(line)) = (failure["source"].as_str(), failure["line"].as_i64())
                && line > 0
                && project(&format!("{source}:{line}"))
            {
                out += &format!("{}\n", paint("2", &format!("      {source}:{line}")));
            }
            for frame in failure["trace"].as_str().unwrap_or("").lines().filter(|l| project(l)) {
                out += &format!("{}\n", paint("2", &format!("        {frame}")));
            }
        }
    }

    if !list("skipped").is_empty() {
        out += &format!("\n{}\n", paint("1;33", &format!("Skipped ({})", list("skipped").len())));
        for skip in list("skipped") {
            let reason = paint("2", &format!("— {}", skip["reason"].as_str().unwrap_or("skipped")));
            out += &format!("  {} {} {reason}\n", paint("33", "○"), labels(skip));
        }
    }

    let mut slowest: Vec<&Value> = list("tests").iter().collect();
    slowest.sort_by(|a, b| seconds(b, "duration_seconds").total_cmp(&seconds(a, "duration_seconds")));
    slowest.truncate(5);
    if slowest.first().is_some_and(|test| seconds(test, "duration_seconds") >= 0.01) {
        out += &format!("\n{}\n", paint("1", &format!("Slowest tests ({})", slowest.len())));
        for test in slowest {
            out += &format!(
                "  {}  {}\n",
                paint("33", &format!("{:>10}", duration(seconds(test, "duration_seconds")))),
                labels(test)
            );
        }
    }

    if !list("benchmarks").is_empty() {
        let mut benches: Vec<&Value> = list("benchmarks").iter().collect();
        benches.sort_by_cached_key(|bench| labels(bench));
        out += &format!("\n{}\n", paint("1;36", &format!("Benchmarks ({})", benches.len())));
        for bench in benches {
            let mean = seconds(bench, "mean_seconds");
            let rate = if mean > 0.0 { format!("{:.2} ops/s", 1.0 / mean) } else { "—".to_owned() };
            let (iterations, samples) =
                (bench["iterations"].as_u64().unwrap_or(0), bench["samples"].as_u64().unwrap_or(1));
            // per-sample iterations; `~` when explicit iterations do not divide evenly over the samples
            let even = if iterations % samples.max(1) == 0 { "" } else { "~" };
            out += &format!(
                "  {}\n      {} mean · {} median · {}..{} · {rate} · ±{:.1}% · {even}{}×{samples}\n",
                labels(bench),
                paint("36", &duration(mean)),
                duration(seconds(bench, "median_seconds")),
                duration(seconds(bench, "min_seconds")),
                duration(seconds(bench, "max_seconds")),
                seconds(bench, "margin_percent"),
                iterations / samples.max(1),
            );
        }
    }

    out.push('\n');
    if report["selected"].as_u64() == Some(0) {
        out += &format!("{}\n", paint("33", &crate::report::none_selected(report)));
        return out;
    }
    let count = |key: &str| report[key].as_u64().unwrap_or(0);
    let tally = |(kind, name): (&str, &str)| {
        let mut parts = Vec::new();
        let (fail, pass, skip) =
            (count(&format!("{kind}_fail")), count(&format!("{kind}_pass")), count(&format!("{kind}_skip")));
        if fail > 0 {
            parts.push(paint("1;31", &format!("{fail} failed")));
        }
        if pass > 0 {
            parts.push(paint("32", &format!("{pass} passed")));
        }
        if skip > 0 {
            parts.push(paint("33", &format!("{skip} skipped")));
        }
        parts.push(paint("2", &format!("{} total", count(&format!("{kind}_total")))));
        format!("{name}: {}", parts.join(&paint("2", " · ")))
    };
    let kinds = [("test", "tests"), ("bench", "benches")];
    let totals: Vec<String> =
        kinds.into_iter().filter(|(kind, _)| count(&format!("{kind}_total")) > 0).map(tally).collect();
    let mark = if report["ok"] == true { paint("32", "✓") } else { paint("31", "✗") };
    out += &format!("{mark} {}\n", totals.join(&paint("2", "  |  ")));
    let focused = report["focused"] == true;
    let summary = format!(
        "  {}/{} selected{} · {} · {ran}",
        count("selected"),
        count("discovered"),
        if focused { " · focused run" } else { "" },
        duration(seconds(report, "duration_seconds")),
    );
    out += &format!("{}\n", paint(if focused { "33" } else { "2" }, &summary));
    out
}

/// A case's labels joined by ` / `, the way filters see them.
fn labels(case: &Value) -> String {
    let names: Vec<&str> = case["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    names.join(" / ")
}

fn seconds(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(0.0)
}

/// `1.23 s`, `4.56 ms`, `7.89 µs` or `12.00 ns`.
fn duration(seconds: f64) -> String {
    match seconds {
        s if s >= 1.0 => format!("{s:.2} s"),
        s if s >= 1e-3 => format!("{:.2} ms", s * 1e3),
        s if s >= 1e-6 => format!("{:.2} µs", s * 1e6),
        s => format!("{:.2} ns", s * 1e9),
    }
}
