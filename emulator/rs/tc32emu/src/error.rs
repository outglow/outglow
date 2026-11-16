use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    WatchdogReset,
    SoftwareReset,
    Emu(Box<String>),
    Stop(Box<String>),
    Other(Box<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WatchdogReset => f.write_str("watchdog reset"),
            Error::SoftwareReset => f.write_str("software reset"),
            Error::Emu(m) => f.write_str(m.as_str()),
            Error::Stop(r) => write!(f, "stop {}", r),
            Error::Other(s) => f.write_str(s.as_str()),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn is_stop(&self) -> bool {
        matches!(self, Error::Stop(_))
    }
    pub fn is_emu(&self) -> bool {
        matches!(self, Error::Emu(_))
    }
}

#[cold]
pub fn emu_err(msg: impl Into<String>) -> Error {
    Error::Emu(Box::new(msg.into()))
}

pub fn stop_err(reason: impl Into<String>) -> Error {
    Error::Stop(Box::new(reason.into()))
}

pub fn other_err(text: impl Into<String>) -> Error {
    Error::Other(Box::new(text.into()))
}
