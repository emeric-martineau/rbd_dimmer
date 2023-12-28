//! RdbDimmer manager
//!
//! RBDDimmer of [RoboDyn](https://robotdyn.fr.aliexpress.com) is device build around two triac.
//!
//! This crate works like official library.
//!
//! You create a dimmer and set a % of time of full power.
//! Device use MOC3021 triac to limit power-lost.
//!
//! You can use `zc` sub-module that manage % by using half sinusoidal.
//! The `zc` sub-module works only for 50Hz voltage.
//! 50Hz = 100 half sinusoidal per seconde => 100%
use esp_idf_hal::gpio::{AnyInputPin, AnyOutputPin, Input, Output, PinDriver};
use esp_idf_hal::task::block_on;
use esp_idf_svc::timer::{EspISRTimerService, EspTimer};
use core::fmt;
use std::time::Duration;

use crate::error::*;

pub mod error;
pub mod zc;

/// Output pin (dimmer).
pub type OutputPin = PinDriver<'static, AnyOutputPin, Output>;
/// Input pin (zero crossing).
pub type InputPin = PinDriver<'static, AnyInputPin, Input>;

/// This enum represent the frequency electricity.
#[derive(Debug, Clone, PartialEq)]
pub enum Frequency {
    /// Voltage has 50Hz frequency (like Europe).
    F50HZ,
    /// Voltage haz 60Hz frequency (like U.K.).
    F60HZ,
}

// Similarly, implement `Display` for `Point2D`.
impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Frequency::F50HZ => write!(f, "50Hz"),
            _ => write!(f, "60Hz"),
        }
    }
}

/// Struct to manage power of dimmer device.
pub struct DimmerDevice {
    id: u8,
    pin: OutputPin,
    invert_power: u8,
}

impl DimmerDevice {
    /// Create new struct.
    pub fn new(id: u8, pin: OutputPin) -> Self {
        DimmerDevice {
            id,
            pin,
            invert_power: 100,
        }
    }

    /// Set power of device. Power is percent of time of half sinusoidal (not of power).
    pub fn set_power(&mut self, p: u8) {
        // It's easy to turn on triac but hard to turn off when voltage > 0.
        // Triac automatically turn off when voltage = 0.
        // At first time of half sinusoidal, we keep off triac and turn on after.
        // That why, we invert power.
        self.invert_power = 100 - p;
    }

    /// Value of tick increase by ISR interrupt. Frequency depends on frequency electricity.
    pub fn tick(&mut self, t: u8) -> Result<(), RbdDimmerError> {
        // If power percent is mower, shutdown pin
        if t >= self.invert_power {
            match self.pin.set_high() {
                Ok(_) => Ok(()),
                Err(_) => Err(RbdDimmerError::from(RbdDimmerErrorKind::SetLow)),
            }
        } else {
            match self.pin.set_low() {
                Ok(_) => Ok(()),
                Err(_) => Err(RbdDimmerError::from(RbdDimmerErrorKind::SetHigh)),
            }
        }
    }

    /// Reset pin to low.
    pub fn reset(&mut self) {
        let _ = self.pin.set_low();
    }
}

/// Manager of dimmer and timer. This is a singleton.
pub struct DevicesDimmerManager {
    // Pin to know if Zero Crossing
    zero_crossing_pin: InputPin
}

impl DevicesDimmerManager {
    /// At first time, init the manager singleton. Else, return singleton already created.
    /// The list of device is singleton.
    pub fn init(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
    ) -> &'static mut Self {
        unsafe {
            match DEVICES_DIMMER_MANAGER.as_mut() {
                None => Self::initialize(zero_crossing_pin, devices, frequency),
                Some(d) => d,
            }
        }
    }

    /// This function wait zero crossing. Zero crossing is low to high impulsion.
    pub fn wait_zero_crossing(&mut self) -> Result<(), RbdDimmerError> {
        let result = block_on(self.zero_crossing_pin.wait_for_rising_edge());

        match result {
            Ok(_) => {
                unsafe {
                    IS_ZERO_CROSSING = true;
                }
                Ok(())
            },
            Err(_) => Err(RbdDimmerError::other(String::from(
                "Fail to wait signal on Zero Cross pin",
            ))),
        }
    }

    /// Set power of a device. The list of device is singleton.
    pub fn set_power(id: u8, power: u8) -> Result<(), RbdDimmerError> {
        unsafe {
            match DIMMER_DEVICES.iter_mut().find(|d| d.id == id) {
                None => Err(RbdDimmerError::from(RbdDimmerErrorKind::DimmerNotFound)),
                Some(device) => {
                    device.set_power(power);
                    Ok(())
                },
            }
        }
    }

    fn initialize(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
    ) -> &'static mut Self {
        unsafe {
            // Copy all devices
            for d in devices {
                DIMMER_DEVICES.push(d);
            }

            ESP_TIMER_SERVICE = Some(EspISRTimerService::new().unwrap());

            let callback = || {
                if TICK > TICK_MAX {
                    IS_ZERO_CROSSING = false;
                    TICK = 0;

                    for d in DIMMER_DEVICES.iter_mut() {
                        d.reset();
                    }
                }

                if IS_ZERO_CROSSING {
                    for d in DIMMER_DEVICES.iter_mut() {
                        // TODO check error or not?
                        let _ = d.tick(TICK);
                    }
    
                    TICK += 1;
                }
            };

            ESP_TIMER = Some(ESP_TIMER_SERVICE.as_ref().unwrap().timer(callback).unwrap());

            let f = match frequency {
                Frequency::F50HZ => HZ_50_DURATION,
                _ => HZ_60_DURATION,
            };

            let _ = ESP_TIMER.as_ref().unwrap().every(f);

            // Create New device manager
            DEVICES_DIMMER_MANAGER = Some(Self {
                zero_crossing_pin
            });

            DEVICES_DIMMER_MANAGER.as_mut().unwrap()
        }
    }
}

// Duration of each percent cycle.
// 50Hz => half sinusoidal / 100 = 0.1 ms
const HZ_50_DURATION: Duration = Duration::from_micros(100);
// 60Hz => half sinusoidal / 100 = 0.083 ms
const HZ_60_DURATION: Duration = Duration::from_micros(83);

static mut IS_ZERO_CROSSING: bool = false;
// List of manager devices
static mut DIMMER_DEVICES: Vec<DimmerDevice> = vec![];
// Timer creator
static mut ESP_TIMER_SERVICE: Option<EspISRTimerService> = None;
// The timer that manager Triac
static mut ESP_TIMER: Option<EspTimer<'static>> = None;
// The device manager
static mut DEVICES_DIMMER_MANAGER: Option<DevicesDimmerManager> = None;
// Tick of device timer counter
static mut TICK: u8 = 0;
// Maximal tick value. Cannot work 100% because of the zero crossing detection timer on the same core.
const TICK_MAX: u8 = 99;
