// Pure notice presentation state; milliseconds are monotonic presentation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Large,
    Capsule,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    pub mode: Mode,
    pub fraction: f64,
    pub urgent: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NoticeState {
    total: u64,
    previous: u64,
    expanded_until_ms: u64,
}

// Everything that can affect pixels in either notice or badge. Kept independent of Win32
// so redraw decisions can be exercised on non-Windows hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderSnapshot {
    pub badge: bool,
    pub title: String,
    pub message: String,
    pub seconds: u64,
    pub mode: Mode,
    pub fraction_bits: u64,
    pub urgent: bool,
    pub width: i32,
    pub height: i32,
    pub dpi: i32,
    pub work_width: i32,
}

pub fn render_changed(previous: Option<&RenderSnapshot>, next: &RenderSnapshot) -> bool {
    previous != Some(next)
}

impl NoticeState {
    pub fn new(remaining: u64, now_ms: u64) -> Self {
        Self {
            total: remaining,
            previous: remaining,
            expanded_until_ms: now_ms.saturating_add(8000),
        }
    }

    // A correction to the countdown never resets the initial duration or expansion timer.
    pub fn step(&mut self, remaining: u64, now_ms: u64, message_changed: bool) -> Presentation {
        if message_changed
            || (self.previous > 300 && remaining <= 300)
            || (self.previous > 60 && remaining <= 60)
        {
            self.expanded_until_ms = now_ms.saturating_add(8000);
        }
        self.previous = remaining;
        Presentation {
            mode: if remaining < 60 || now_ms < self.expanded_until_ms {
                Mode::Large
            } else {
                Mode::Capsule
            },
            fraction: if self.total == 0 {
                0.0
            } else {
                (remaining as f64 / self.total as f64).clamp(0.0, 1.0)
            },
            urgent: remaining < 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraw_only_when_visible_content_changes() {
        let base = RenderSnapshot {
            badge: false,
            title: "Fermeture".into(),
            message: "Préavis".into(),
            seconds: 60,
            mode: Mode::Large,
            fraction_bits: 0.5_f64.to_bits(),
            urgent: false,
            width: 300,
            height: 164,
            dpi: 96,
            work_width: 1280,
        };
        assert!(render_changed(None, &base));
        assert!(!render_changed(Some(&base), &base));
        for changed in [
            RenderSnapshot {
                seconds: 59,
                ..base.clone()
            },
            RenderSnapshot {
                message: "Nouveau".into(),
                ..base.clone()
            },
            RenderSnapshot {
                fraction_bits: 0.4_f64.to_bits(),
                ..base.clone()
            },
            RenderSnapshot {
                mode: Mode::Capsule,
                ..base.clone()
            },
            RenderSnapshot {
                urgent: true,
                ..base.clone()
            },
            RenderSnapshot {
                width: 290,
                ..base.clone()
            },
            RenderSnapshot {
                dpi: 144,
                ..base.clone()
            },
            RenderSnapshot {
                work_width: 900,
                ..base.clone()
            },
            RenderSnapshot {
                badge: true,
                ..base.clone()
            },
        ] {
            assert!(render_changed(Some(&base), &changed));
        }
    }

    #[test]
    fn lifecycle_corrections_thresholds_and_message() {
        let mut state = NoticeState::new(360, 0);
        assert_eq!(state.step(360, 0, false).mode, Mode::Large);
        assert_eq!(state.step(352, 8000, false).mode, Mode::Capsule);
        // Correction from service, including an increased remaining time, preserves initial total.
        assert_eq!(state.step(355, 8100, false).fraction, 355.0 / 360.0);
        assert_eq!(state.step(300, 10_000, false).mode, Mode::Large);
        assert_eq!(state.step(290, 18_000, false).mode, Mode::Capsule);
        assert_eq!(state.step(289, 19_000, true).mode, Mode::Large);
        assert_eq!(state.step(280, 27_000, false).mode, Mode::Capsule);
        assert_eq!(state.step(60, 300_000, false).mode, Mode::Large);
        assert_eq!(state.step(60, 308_000, false).mode, Mode::Capsule);
        let urgent = state.step(59, 309_000, false);
        assert_eq!(urgent.mode, Mode::Large);
        assert!(urgent.urgent);
        assert_eq!(state.step(0, 400_000, false).fraction, 0.0);
    }

    #[test]
    fn short_and_skipped_thresholds() {
        let mut short = NoticeState::new(30, 0);
        assert_eq!(short.step(25, 100_000, false).mode, Mode::Large);
        let mut skipped = NoticeState::new(400, 0);
        assert_eq!(skipped.step(55, 100_000, false).mode, Mode::Large);
        assert!(skipped.step(55, 110_000, false).urgent);
        let mut zero = NoticeState::new(0, 0);
        assert_eq!(zero.step(0, 0, false).fraction, 0.0);
    }
}
