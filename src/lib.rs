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
use std::cell::RefCell;
use esp_idf_hal::gpio::{AnyInputPin, AnyOutputPin, Input, Output, PinDriver};
use esp_idf_hal::task::block_on;
use esp_idf_svc::timer::{EspISRTimerService, EspTimer};
use esp_idf_sys::EspError;
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU8, Ordering as aOrdering};
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
// Maximal tick value. Cannot work 100% because of the zero crossing detection timer on the same core.
static TICK_MAX: AtomicU8 = AtomicU8::new(95);
// Tick of device timer counter. TICK=0 means zero crossing detected.
// If TICK=TICK_MAX, nothing happen.
static TICK: AtomicU8 = AtomicU8::new(0);
// Step of tick
static TICK_STEP: AtomicU8 = AtomicU8::new(1);

/// Output pin (dimmer).
pub type OutputPin = PinDriver<'static, AnyOutputPin, Output>;
/// Input pin (zero crossing).
pub type InputPin = PinDriver<'static, AnyInputPin, Input>;

struct GlobalDimmerManager {
    // List of manager devices
    devices: RefCell<Vec<DimmerDevice>>,
    // The device manager
    manager: RefCell<Option<DevicesDimmerManager>>,
}

unsafe impl Sync for GlobalDimmerManager {
    
}

static GLOBAL_DIMMER_INSTANCE: GlobalDimmerManager = GlobalDimmerManager {
    devices: RefCell::new(vec![]),
    manager: RefCell::new(None),
};

/// This enum represent the frequency electricity.
#[derive(Debug, Clone, PartialEq)]
pub enum Frequency {
    /// Voltage has 50Hz frequency (like Europe).
    F50HZ,
    /// Voltage haz 60Hz frequency (like U.K.).
    F60HZ,
}

/// Similarly, implement `Display` for `Frequency`.
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
        // In case of we have 100% of power, we never reset.
        if self.invert_power > 0 {
            let _ = self.pin.set_low();
        }
    }
}

unsafe impl Sync for DimmerDevice {

}

/// Config of device manager
pub struct DevicesDimmerManagerConfig {
    /// Pin for read zero crossing
    pub zero_crossing_pin: InputPin,
    /// List of devices to manage
    pub devices: Vec<DimmerDevice>,
    /// Frequency of network (Europe = 50Hz)
    pub frequency: Frequency,
    /// Step of manage power. In 50Hz, by default, power is managed
    /// every 0.1ms. But you can multiy by step_size.
    /// That mean is step_size = 10, power management is every 1ms and
    /// power tick is also multiply by 10 (power step wil by 0, 10, 20, 30...)
    pub step_size: u8,
    /// Tick max of power management in percent.
    /// By default, you cannot set power more than 95%.
    pub tick_max: u8,
}

impl DevicesDimmerManagerConfig {
    pub fn default(
        zero_crossing_pin: InputPin,
        devices: Vec<DimmerDevice>,
        frequency: Frequency,
    ) -> Self {
        Self {
            zero_crossing_pin,
            devices,
            frequency,
            step_size: 1,
            tick_max: 95,
        }
    }

    pub fn default_50_hz(zero_crossing_pin: InputPin, devices: Vec<DimmerDevice>) -> Self {
        Self {
            zero_crossing_pin,
            devices,
            frequency: Frequency::F50HZ,
            step_size: 1,
            tick_max: 95,
        }
    }

    pub fn default_60_hz(zero_crossing_pin: InputPin, devices: Vec<DimmerDevice>) -> Self {
        Self {
            zero_crossing_pin,
            devices,
            frequency: Frequency::F60HZ,
            step_size: 1,
            tick_max: 95,
        }
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
    /// At first time, init the manager singleton.
    pub fn init(
        config: DevicesDimmerManagerConfig,
    ) -> Result<(), RbdDimmerError> {
        TICK_MAX.store(config.tick_max, aOrdering::Relaxed);
        TICK.store(config.tick_max, aOrdering::Relaxed);

        match Self::initialize(config) {
            Ok(d) => Ok(d),
            Err(e) => Err(RbdDimmerError::new(
                RbdDimmerErrorKind::Other,
                format!("Fail to initialize timer. Error code: {}", e),
            )),
        }
    }

    /// This function wait zero crossing. Zero crossing is low to high impulsion.
    #[inline(always)]
    fn wait_zero_crossing(&mut self) -> Result<(), RbdDimmerError> {
        let result = block_on(self.zero_crossing_pin.wait_for_falling_edge());

        match result {
            Ok(_) => {
                TICK.store(0, aOrdering::Relaxed);
                Ok(())
            }
            Err(_) => Err(RbdDimmerError::other(String::from(
                "Fail to wait signal on Zero Cross pin",
            ))),
        }
    }

    /// Stop timer
    fn stop(&self) -> Result<bool, RbdDimmerError> {
        TICK.store(TICK_MAX.load(aOrdering::Relaxed), aOrdering::Relaxed);

        match self.esp_timer.cancel() {
            Ok(status) => Ok(status),
            Err(e) => Err(RbdDimmerError::new(
                RbdDimmerErrorKind::TimerCancel,
                format!("Fail to stop timer. Error code: {}", e),
            )),
        }
    }

    fn initialize(config: DevicesDimmerManagerConfig) -> Result<(), EspError> {
        unsafe {
            {       
                let mut devices = GLOBAL_DIMMER_INSTANCE.devices.borrow_mut();

                for d in config.devices {
                    devices.push(d);
                }
            } // Borrom mut is release here

            TICK_STEP.store(config.step_size, aOrdering::Relaxed);

            let callback = || {
                let tick_max = TICK_MAX.load(aOrdering::Relaxed);
                let tick = TICK.load(aOrdering::Relaxed);
                match tick.cmp(&tick_max) {
                    Ordering::Less => {
                        match GLOBAL_DIMMER_INSTANCE.devices.try_borrow_mut() {
                            Ok(mut devices) => {
                                for d in devices.iter_mut() {
                                    // TODO check error or not?
                                    let _ = d.tick(TICK.load(aOrdering::Relaxed));
                                }
                            },
                            Err(_) => {},
                        }

                        TICK.store(
                            tick + TICK_STEP.load(aOrdering::Relaxed),
                            aOrdering::Relaxed,
                        );
                    }
                    Ordering::Greater => {}
                    Ordering::Equal => {
                        match GLOBAL_DIMMER_INSTANCE.devices.try_borrow_mut() {
                            Ok(mut devices) => {
                                for d in devices.iter_mut() {
                                    d.reset();
                                }
                            },
                            Err(_) => {},
                        }
                    }
                };
            };

            // Timer creator
            let esp_timer_service = EspISRTimerService::new()?;
            let esp_timer = esp_timer_service.timer(callback)?;

            let f = match config.frequency {
                Frequency::F50HZ => HZ_50_DURATION,
                _ => HZ_60_DURATION,
            };

            esp_timer.every(Duration::from_micros(
                (f as u64) * (config.step_size as u64),
            ))?;

            // Create New device manager
            let mut manager = GLOBAL_DIMMER_INSTANCE.manager.borrow_mut();

            *manager = Some(Self {
                zero_crossing_pin: config.zero_crossing_pin,
                esp_timer,
            });

            Ok(())
        }
    }
}

/// Set power of a device. The list of device is singleton.
pub fn set_power(id: u8, power: u8) -> Result<(), RbdDimmerError> {
    match GLOBAL_DIMMER_INSTANCE.devices.try_borrow_mut() {
        Ok(mut devices) => {
            match devices.iter_mut().find(|d| d.id == id) {
                Some(device) => {
                    device.set_power(power);
                    Ok(())
                },
                None => Err(RbdDimmerError::from(RbdDimmerErrorKind::DimmerNotFound)),
            }
        },
        Err(_) => Ok(()),
    }
}

/// Stop manager.
pub fn stop() -> Result<bool, RbdDimmerError> {
    match GLOBAL_DIMMER_INSTANCE.manager.try_borrow_mut() {
        Ok(mut manager) => {
            match manager.as_mut() {
                Some(d) => d.stop(),
                None => Err(RbdDimmerError::from(RbdDimmerErrorKind::DimmerManagerNotInit)),
            }
        },
        Err(_) => Ok(false),
    }
}

pub fn wait_zero_crossing() ->  Result<bool, RbdDimmerError> {
    match GLOBAL_DIMMER_INSTANCE.manager.try_borrow_mut() {
        Ok(mut manager) => {
            match manager.as_mut() {
                Some(d) => {
                    match d.wait_zero_crossing() {
                        Ok(()) => Ok(true),
                        Err(e) => Err(e),
                    }
                },
                None => Err(RbdDimmerError::from(RbdDimmerErrorKind::DimmerManagerNotInit)),
            }
        },
        Err(_) => Ok(false),
    }
}