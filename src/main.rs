//! Dumps every metric as JSON. `cargo run -- [path]` samples once over a
//! second and writes `metrics.json` (or `path`), then lists the metrics that
//! came back empty. `cargo run -- --watch [interval_ms]` runs the
//! [`Tracker`] instead and prints one JSON line per reading until stopped.

use std::error::Error;
use std::fs::File;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use top_metrics::tracker::types::constants::DEFAULT_INTERVAL;
use top_metrics::{Metric, Reading, ThermalPressure, Tracker, TrackerConfig};

/// The span the one-shot dump's rates (CPU, GPU, power, network, disk,
/// processes) cover.
const INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--watch") => watch(args.next().map(parse_millis).transpose()?),
        path => dump(path.unwrap_or("metrics.json")),
    }
}

/// One sample over `INTERVAL`, pretty-printed to `path`, then the metrics
/// that came back empty.
fn dump(path: &str) -> Result<(), Box<dyn Error>> {
    let report = sample_once();
    let mut file = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.write_all(b"\n")?;
    file.flush()?;
    println!("wrote {path}");

    let mut missing = Vec::new();
    nulls(&report, "", &mut missing);
    if missing.is_empty() {
        println!("every metric was read");
    } else {
        println!("not read: {}", missing.join(", "));
    }
    Ok(())
}

/// The chip and one snapshot taken `INTERVAL` after the baselines, with the
/// processes sorted by CPU.
fn sample_once() -> Value {
    // Loads the GPU driver's library, opens RAPL and takes the baselines the
    // rates need.
    let mut sampler = top_metrics::Sampler::new();
    std::thread::sleep(INTERVAL);
    let mut snapshot = sampler.sample();
    if let Some(processes) = &mut snapshot.processes {
        processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    }
    json!({
        "interval_secs": INTERVAL.as_secs_f64(),
        "chip": sampler.chip(),
        "cpu_usage": snapshot.cpu.usage(),
        "power_total": snapshot.power.total(),
        "snapshot": snapshot,
    })
}

/// The `--watch` interval argument, named in the error a bad one gives.
fn parse_millis(arg: String) -> Result<u64, String> {
    arg.parse().map_err(|error| {
        format!("--watch interval_ms: {arg:?} is not a count of milliseconds ({error})")
    })
}

/// The tracker's readings as JSON lines on stdout, processes included, until
/// Ctrl-C or the reader of the pipe goes away.
fn watch(interval_ms: Option<u64>) -> Result<(), Box<dyn Error>> {
    let tracker = Tracker::start(TrackerConfig {
        interval: interval_ms.map_or(DEFAULT_INTERVAL, Duration::from_millis),
        ..TrackerConfig::default()
    })?;
    let _processes = tracker.demand_processes();
    let updates = tracker.subscribe();
    let mut out = BufWriter::new(io::stdout());
    for reading in updates {
        if !emit(&mut out, &line(&reading, tracker.interval()))? {
            break;
        }
    }
    tracker.stop();
    Ok(())
}

/// Writes one line and flushes it; `false` once the reader of the pipe has
/// gone away, which ends the stream rather than failing it.
fn emit(out: &mut impl Write, value: &Value) -> io::Result<bool> {
    match writeln!(out, "{value}").and_then(|()| out.flush()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error),
    }
}

/// One reading as a flat JSON object: every metric's value, window mean and
/// peak, the thermal pressure and the busiest processes.
fn line(reading: &Reading, interval: Duration) -> Value {
    json!({
        "sequence": reading.sequence(),
        "taken_at": reading
            .taken_at()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64()),
        "interval_secs": interval.as_secs_f64(),
        "metrics": metric_entries(reading),
        "thermal": reading.snapshot().thermal.map(ThermalPressure::label),
        "busiest": busiest_entries(reading),
    })
}

/// Every metric by its label: the latest value, the window's mean, the peak
/// since start and the unit.
fn metric_entries(reading: &Reading) -> Map<String, Value> {
    Metric::ALL
        .iter()
        .map(|&metric| {
            let series = reading.history().series(metric);
            let entry = json!({
                "value": series.latest(),
                "mean": series.mean(),
                "peak": series.peak(),
                "unit": metric.unit(),
            });
            (metric.label().to_owned(), entry)
        })
        .collect()
}

/// The busiest processes, most CPU first.
fn busiest_entries(reading: &Reading) -> Vec<Value> {
    reading
        .busiest()
        .map(|process| {
            json!({
                "pid": process.pid,
                "command": process.command,
                "cpu": process.cpu,
                "memory": process.memory,
            })
        })
        .collect()
}

/// Collects the paths of `null` fields, the metrics that couldn't be read.
/// Lists aren't walked, so one odd process or volume can't flood the summary.
fn nulls(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Null => out.push(path.to_owned()),
        Value::Object(fields) => {
            for (key, field) in fields {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                nulls(field, &path, out);
            }
        }
        _ => {}
    }
}
