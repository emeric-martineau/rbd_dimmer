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
use core::fmt;
use esp_idf_hal::gpio::{AnyInputPin, AnyOutputPin, Input, Output, PinDriver};
use esp_idf_hal::task::block_on;
use esp_idf_svc::timer::{EspISRTimerService, EspTimer};
use esp_idf_sys::EspError;
use std::time::Duration;

use crate::error::*;

pub mod error;
pub mod zc;

//---------------------------------------------------------------------------------------
// Explanation of the internal workings
// ====================================
//
// An item on AC network have a global power characteristic. This power is not instant
// power. This power is P= U.I.cos φ.
// Where:
//  - U = Umax / √2
//  - I = Imax / √2
// That mean, if you cut voltage on half of half sinusoidal. Item use only 50% of
// official power.
//
// To do that, we use a triac MOC2031 and we turn on this triac only we it's necessary.
//
// To do that, we need detect the zero crossing and use timer to cut half sinusoidal in
// 100.
//
// In 50Hz, we have only 10ms for one half sinusoidal! That mean, each part of 100 has
// only 0.1ms!!!
//
// In this case, we need do the job very quicly. And to do that, Rust is not really
// helpfull :)
// We need use ISR timer. That mean we cannot have context. We need use static global
// variable.
//
// The ISR timer is always on.
// When zero crossing is detected, we set IS_ZERO_CROSSING to true.
// When IS_ZERO_CROSSING is true, ISR timer increase TICK from 0 to TICK_MAX (normaly
// 100 but in this case, we have collision with zero crossing detection).
// The ISR timer call `tick()` method of each dimmer.
//---------------------------------------------------------------------------------------

// Duration of each percent cycle.
// 50Hz => half sinusoidal / 100 = 0.1 ms
const HZ_50_DURATION: u8 = 100;
// 60Hz => half sinusoidal / 100 = 0.083 ms
const HZ_60_DURATION: u8 = 83;
// List of manager devices
static mut DIMMER_DEVICES: Vec<DimmerDevice> = vec![];

// The device manager
static mut DEVICES_DIMMER_MANAGER: Option<DevicesDimmerManager> = None;
// Maximal tick value. Cannot work 100% because of the zero crossing detection timer on the same core.
const TICK_MAX: u8 = 98;
// Tick of device timer counter. TICK=0 means zero crossing detected.
// If TICK=TICK_MAX, nothing happen.
static mut TICK: u8 = TICK_MAX;
// Step of tick
static mut TICK_STEP: u8 = 1;


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
    #[inline(always)]
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
    #[inline(always)]
    pub fn reset(&mut self) {
        let _ = self.pin.set_low();
    }
}

/// Manager of dimmer and timer. This is a singleton.
pub struct DevicesDimmerManager {
    // Pin to know if Zero Crossing
    zero_crossing_pin: InputPin,
    // The timer that manager Triac
    esp_timer: EspTimer<'static>,
}

impl DevicesDimmerManager {
    /// At first time, init the manager singleton. Else, return singleton already created.
    /// The list of device is singleton.
    pub fn init(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
    ) -> Result<&'static mut Self, RbdDimmerError> {
        Self::init_advanced(zero_crossing_pin, devices, frequency, 1)
    }

    /// At first time, init the manager singleton. Else, return singleton already created.
    /// The list of device is singleton.
    /// step_size is allow to contol if power management is do 0 to 100 by step_size.
    /// That allow also to be create more time for ISR timer (in 50Hz always 0.1ms * step size ).
    pub fn init_advanced(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
        step_size: u8,
    ) -> Result<&'static mut Self, RbdDimmerError> {
        unsafe {
            match DEVICES_DIMMER_MANAGER.as_mut() {
                None => match Self::initialize(zero_crossing_pin, devices, frequency, step_size) {
                    Ok(d) => Ok(d),
                    Err(e) => Err(RbdDimmerError::new(
                        RbdDimmerErrorKind::Other,
                        format!("Fail to initialize timer. Error code: {}", e),
                    )),
                },
                Some(d) => Ok(d),
            }
        }
    }

    /// This function wait zero crossing. Zero crossing is low to high impulsion.
    #[inline(always)]
    pub fn wait_zero_crossing(&mut self) -> Result<(), RbdDimmerError> {
        let result = block_on(self.zero_crossing_pin.wait_for_falling_edge());

        match result {
            Ok(_) => {
                unsafe {
                    for d in DIMMER_DEVICES.iter_mut() {
                        d.reset();
                    }

                    TICK = 0;
                }
                Ok(())
            }
            Err(_) => Err(RbdDimmerError::other(String::from(
                "Fail to wait signal on Zero Cross pin",
            ))),
        }
    }

    /// Stop timer
    pub fn stop(&mut self) -> Result<bool, RbdDimmerError> {
        unsafe {
            TICK = TICK_MAX;
        }

        match self.esp_timer.cancel() {
            Ok(status) => Ok(status),
            Err(e) => Err(RbdDimmerError::new(
                RbdDimmerErrorKind::TimerCancel,
                format!("Fail to stop timer. Error code: {}", e),
            )),
        }
    }

    fn initialize(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
        step_size: u8,
    ) -> Result<&'static mut Self, EspError> {
        unsafe {
            // Copy all devices
            for d in devices {
                DIMMER_DEVICES.push(d);
            }

            TICK_STEP = step_size;

            let callback = || {
                if TICK < TICK_MAX {
                    for d in DIMMER_DEVICES.iter_mut() {
                        // TODO check error or not?
                        let _ = d.tick(TICK);
                    }
                    
                    TICK += TICK_STEP;
                } else if TICK == TICK_MAX {
                    for d in DIMMER_DEVICES.iter_mut() {
                        d.reset();
                    }
                }
            };

            // Timer creator
            let esp_timer_service = EspISRTimerService::new()?;
            let esp_timer = esp_timer_service.timer(callback)?;

            let f = match frequency {
                Frequency::F50HZ => HZ_50_DURATION,
                _ => HZ_60_DURATION,
            };

            esp_timer.every(Duration::from_micros((f as u64) * (step_size as u64)))?;

            // Create New device manager
            DEVICES_DIMMER_MANAGER = Some(Self {
                zero_crossing_pin,
                esp_timer,
            });

            Ok(DEVICES_DIMMER_MANAGER.as_mut().unwrap())
        }
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
            }
        }
    }
}
