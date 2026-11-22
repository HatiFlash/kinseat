#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#[cfg(any(windows, test))]
mod image_processing;
#[cfg(any(windows, test))]
mod layout;
#[cfg(any(windows, test))]
mod notice_state;
#[cfg(any(windows, test))]
mod protocol;
#[cfg(any(windows, test))]
mod ring;
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
fn remaining(initial: u64, since: Instant, now: Instant) -> u64 {
    initial.saturating_sub(now.duration_since(since).as_secs())
}
#[cfg(any(windows, test))]
fn urgent(seconds: u64) -> bool {
    seconds < 60
}
#[cfg(any(windows, test))]
fn dimensions(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    if width <= max_width || max_width == 0 {
        (width, height)
    } else {
        (
            max_width,
            ((height as u64 * max_width as u64 + width as u64 / 2) / width as u64).max(1) as u32,
        )
    }
}

#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    {
        win::log(&format!(
            "start version={} pid={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
        std::panic::set_hook(Box::new(|info| {
            // Panic payloads can contain user data; record the location only.
            let location = info.location().map_or("unknown".to_string(), |l| {
                format!("{}:{}", l.file(), l.line())
            });
            win::log(&format!("panic at {location}"));
        }));
    }
    if let Err(e) = run() {
        #[cfg(windows)]
        win::log(&format!("exit error: {e}"));
        eprintln!("kinseat-agent : {e}");
        std::process::exit(1);
    }
    #[cfg(windows)]
    win::log("exit normal");
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--version") {
        println!("kinseat-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(windows)]
    let result = win::run(&args);
    #[cfg(not(windows))]
    let result = {
        let _ = args;
        Err("l'agent de session nécessite Windows".into())
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timing_and_color() {
        let start = Instant::now();
        assert_eq!(remaining(61, start, start + Duration::from_secs(2)), 59);
        assert!(urgent(59));
        assert!(!urgent(60));
        assert_eq!(remaining(1, start, start + Duration::from_secs(10)), 0);
    }
    #[test]
    fn ratio() {
        assert_eq!(dimensions(1920, 1080, 1280), (1280, 720));
        assert_eq!(dimensions(800, 600, 1280), (800, 600));
    }
}
