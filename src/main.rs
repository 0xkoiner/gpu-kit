//! Samples every metric over one second and writes them as JSON:
//! `cargo run -- [path]`, to `metrics.json` by default. Then lists the
//! metrics that came back empty, which this machine or this run couldn't read.

use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Duration;

use serde_json::{Value, json};

/// The span the rates (CPU, GPU, power, network, disk, processes) cover.
const INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "metrics.json".to_owned());

    // Loads the GPU driver's library, opens RAPL and takes the baselines the
    // rates need.
    let mut sampler = top_metrics::Sampler::new();
    std::thread::sleep(INTERVAL);
    let mut snapshot = sampler.sample();
    if let Some(processes) = &mut snapshot.processes {
        processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    }

    let report = json!({
        "interval_secs": INTERVAL.as_secs_f64(),
        "chip": sampler.chip(),
        "cpu_usage": snapshot.cpu.usage(),
        "power_total": snapshot.power.total(),
        "snapshot": snapshot,
    });

    let mut file = BufWriter::new(File::create(&path)?);
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.write_all(b"\n")?;
    file.flush()?;

    let mut missing = Vec::new();
    nulls(&report, "", &mut missing);
    println!("wrote {path}");
    if missing.is_empty() {
        println!("every metric was read");
    } else {
        println!("not read: {}", missing.join(", "));
    }
    Ok(())
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
