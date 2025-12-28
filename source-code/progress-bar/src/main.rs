use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::io::{self, BufRead};
use std::time::{Duration, Instant};

fn main() {
    let start_time = Instant::now();
    let stdin = io::stdin();
    let mut total: u64 = 0;
    let mut current: u64 = 0;
    let mut message = String::from("Initializing...");

    let m = MultiProgress::new();

    let pb = m.add(ProgressBar::new(total));
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg} ETA: {eta_precise}"
        )
        .unwrap()
        .progress_chars("█▌ ")
        .tick_strings(&["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▁", ""])
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message(message.clone());

    let log_pb = m.add(ProgressBar::new(0));
    log_pb.set_style(
        ProgressStyle::with_template("{msg}")
            .unwrap()
    );
    log_pb.set_message("No logs yet...");

    for line in stdin.lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        if line.starts_with("set_total ") {
            if let Ok(t) = line[10..].parse::<u64>() {
                total = t;
                pb.set_length(total);
            }
        } else if line.starts_with("msg ") {
            message = line[4..].to_string();
            pb.set_message(message.clone());
        } else if line.starts_with("log ") {
            let log_msg = format!("Log: {}", &line[4..]);
            log_pb.set_message(log_msg);
        } else if line.starts_with("error ") {
            let err_msg = format!("Error: {}", &line[6..]);
            log_pb.set_message(err_msg);
        } else if line == "update" {
            current += 1;
            pb.set_position(current);
        } else if line == "done" {
            pb.finish_with_message(format!("Completed in {:.2}s", start_time.elapsed().as_secs_f64()));
            log_pb.finish_and_clear();
            break;
        }
    }
}
