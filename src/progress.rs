use crate::cli::Options;
use anyhow::Result;
use std::{
    io::{self, IsTerminal, Write},
    sync::Mutex,
    time::{Duration, Instant},
};

/// One shared display, including when counting workers finish concurrently.
pub(crate) struct Progress {
    enabled: bool,
    terminal: bool,
    rate: usize,
    throttle: bool,
    display: Mutex<Display>,
}

struct Display {
    phase: &'static str,
    completed: usize,
    total: Option<usize>,
    last_update: Instant,
    active: bool,
}

impl Progress {
    pub(crate) fn new(o: &Options) -> Result<Self> {
        let rate = o.number("progress-rate", 100.0)? as usize;
        let terminal = io::stderr().is_terminal();
        Ok(Self {
            enabled: !o.flag("quiet") && rate > 0 && (terminal || o.has("progress-rate")),
            terminal,
            rate,
            throttle: !o.has("progress-rate"),
            display: Mutex::new(Display {
                phase: "Scanning",
                completed: 0,
                total: None,
                last_update: Instant::now(),
                active: false,
            }),
        })
    }

    pub(crate) fn begin(&self, phase: &'static str, total: Option<usize>) {
        if !self.enabled {
            return;
        }
        let mut display = self.display.lock().unwrap();
        display.phase = phase;
        display.completed = 0;
        display.total = total;
        self.render(&mut display);
    }

    pub(crate) fn update(&self, phase: &'static str, completed: usize, total: Option<usize>) {
        if !self.enabled || completed % self.rate != 0 {
            return;
        }
        let mut display = self.display.lock().unwrap();
        if (self.throttle && display.last_update.elapsed() < Duration::from_millis(500))
            || (phase == display.phase && completed < display.completed)
        {
            return;
        }
        display.phase = phase;
        display.completed = completed;
        display.total = total;
        self.render(&mut display);
    }

    pub(crate) fn finish(&self, completed: usize) {
        if !self.enabled {
            return;
        }
        let mut display = self.display.lock().unwrap();
        display.completed = completed;
        self.render(&mut display);
        if self.terminal {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr);
            let _ = stderr.flush();
        }
        display.active = false;
    }

    fn render(&self, display: &mut Display) {
        let mut stderr = io::stderr().lock();
        if self.terminal {
            let _ = write!(stderr, "\r\x1b[2K");
        }
        let _ = write!(stderr, "{}: {}", display.phase, display.completed);
        if let Some(total) = display.total {
            let _ = write!(stderr, " / {total}");
        }
        let _ = write!(stderr, " files");
        if !self.terminal {
            let _ = writeln!(stderr);
        }
        let _ = stderr.flush();
        display.last_update = Instant::now();
        display.active = true;
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if self.terminal && self.display.get_mut().unwrap().active {
            // Leave diagnostics a clean line if an operation exits early.
            let mut stderr = io::stderr().lock();
            let _ = write!(stderr, "\r\x1b[2K");
            let _ = stderr.flush();
        }
    }
}
