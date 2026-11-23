//! Plain-text log file, recreated on every game start.

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Writes timestamped lines to a file. Silently does nothing if the file can't be created.
pub struct LogFile {
    file: Option<File>,
    start: Instant,
}

/// Writes one formatted line to a [`LogFile`]: `log!(log, "found {x}")`.
macro_rules! log {
    ($log:expr, $($arg:tt)*) => {
        $log.line(format_args!($($arg)*))
    };
}

impl LogFile {
    pub fn create(path: &Path) -> Self {
        Self {
            file: File::create(path).ok(),
            start: Instant::now(),
        }
    }

    pub fn line(&mut self, message: fmt::Arguments<'_>) {
        if let Some(file) = &mut self.file {
            let elapsed = self.start.elapsed().as_secs_f32();
            let _ = writeln!(file, "[{elapsed:7.2}s] {message}");
            let _ = file.flush();
        }
    }
}
