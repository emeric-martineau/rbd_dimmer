//! Error of RBDDimmer struct
use std::fmt;

/// RBDDimmer type of error
#[derive(Debug, Clone, PartialEq)]
pub enum RbdDimmerErrorKind {
    /// When dimmer try set high pin and fail
    SetHigh,
    /// When dimmer try set low pin and fail
    SetLow,
    /// When DevicesDimmerManager lost connection with thread channel
    ChannelCommunicationDisconnected,
    /// Unknow error
    Other,
    /// Timer cancel error
    TimerCancel,
    /// Timer is scheduled
    TimerScheduled,
    /// Timer every
    TimerEvery,
    /// No dimmer found with ID
    DimmerNotFound,
}

/// Uart error with type and message
#[derive(Debug, Clone)]
pub struct RbdDimmerError {
    pub message: String,
    pub kind: RbdDimmerErrorKind,
}

impl fmt::Display for RbdDimmerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Error when use RBDDimmer interface. Reason: {}",
            self.message
        )
    }
}

impl RbdDimmerError {
    pub fn new(kind: RbdDimmerErrorKind, message: String) -> Self {
        Self { message, kind }
    }

    pub fn from(kind: RbdDimmerErrorKind) -> Self {
        Self {
            message: String::new(),
            kind,
        }
    }

    pub fn other(message: String) -> Self {
        Self {
            message,
            kind: RbdDimmerErrorKind::Other,
        }
    }
}
