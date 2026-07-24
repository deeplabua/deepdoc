//! Minimal stderr logging. stdout belongs to the extracted document — never
//! write diagnostics there.

/// How chatty the run is (`-q` / `-v`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Quiet,
    Normal,
    Verbose,
    Debug,
}

impl Level {
    pub fn from_flags(quiet: bool, verbose: u8) -> Level {
        match (quiet, verbose) {
            (true, _) => Level::Quiet,
            (false, 0) => Level::Normal,
            (false, 1) => Level::Verbose,
            (false, _) => Level::Debug,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Logger {
    level: Level,
}

impl Logger {
    pub fn new(level: Level) -> Self {
        Self { level }
    }

    /// Progress information; hidden with `-q`.
    pub fn info(&self, message: impl AsRef<str>) {
        if self.level >= Level::Normal {
            eprintln!("{}", message.as_ref());
        }
    }

    /// Detail; shown from `-v` upwards.
    pub fn verbose(&self, message: impl AsRef<str>) {
        if self.level >= Level::Verbose {
            eprintln!("{}", message.as_ref());
        }
    }

    /// A problem that did not stop the run.
    pub fn warn(&self, message: impl AsRef<str>) {
        if self.level >= Level::Normal {
            eprintln!("warning: {}", message.as_ref());
        }
    }

    /// A failure. Always shown, even with `-q`.
    pub fn error(&self, message: impl AsRef<str>) {
        eprintln!("error: {}", message.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_map_to_levels() {
        assert_eq!(Level::from_flags(true, 0), Level::Quiet);
        assert_eq!(Level::from_flags(false, 0), Level::Normal);
        assert_eq!(Level::from_flags(false, 1), Level::Verbose);
        assert_eq!(Level::from_flags(false, 3), Level::Debug);
    }
}
