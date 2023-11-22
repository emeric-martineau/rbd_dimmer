//! RdbDimmer manager
//!
//! RBDDimmer of [RoboDyn](https://robotdyn.fr.aliexpress.com) is device build around two triac.
//!
//! This crate not works like official library. Power is turn on/off on Zero Crossing event if device has MOC3021 triac to limit power-lost.
//!
use crate::error::*;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::mpsc::{Receiver, Sender};

pub mod error;
#[cfg(test)]
mod tests;

/// Abstract output pin
pub trait OutputPin {
    /// Set the output as high
    fn set_high(&mut self) -> Result<(), RbdDimmerError>;

    /// Set the output as low
    fn set_low(&mut self) -> Result<(), RbdDimmerError>;
}

/// Struct to manage power of dimmer device
pub struct DimmerDevice<O>
where
    O: OutputPin,
{
    id: u8,
    pin: O,
    power: u8,
}

impl<O> DimmerDevice<O>
where
    O: OutputPin,
{
    /// Create new struct
    pub fn new(id: u8, pin: O) -> Self {
        DimmerDevice { id, pin, power: 0 }
    }

    /// Set power of device. Power is percent
    pub fn set_power(&mut self, p: u8) {
        self.power = p;
    }

    /// Value of tick increase by zero crossing interrupt
    pub fn tick(&mut self, t: u8) -> Result<(), RbdDimmerError> {
        // If power percent is over, shutdown pin
        if t > self.power {
            return self.pin.set_low();
        }

        self.pin.set_high()
    }

    pub fn pin(&mut self) -> &O {
        &self.pin
    }
}

/// Struct to communicate with DevicesDimmerManager
#[derive(Debug)]
pub struct DevicesDimmerManagerNotification {
    /// Id of device
    pub id: u8,
    /// New power value
    pub power: u8,
}

/// Zero crossing pin abstract
pub trait ZeroCrossingPin {
    /// Wait for rising
    fn wait_for_rising_edge(&mut self) -> Result<(), RbdDimmerError>;
}

pub struct DevicesDimmerManager<O, ZC>
where
    O: OutputPin,
    ZC: ZeroCrossingPin,
{
    // Devices to manage
    devices: Vec<DimmerDevice<O>>,
    // Pin to know if Zero Crossing
    zero_crossing_pin: ZC,
    // Channel to communicate with thread
    tx_power_change: Sender<DevicesDimmerManagerNotification>,
    rx_power_change: Receiver<DevicesDimmerManagerNotification>,
    // Current counter of zero crossing
    counter: u8,
}

impl<O, ZC> DevicesDimmerManager<O, ZC>
where
    O: OutputPin,
    ZC: ZeroCrossingPin,
{
    pub fn new(zero_crossing_pin: ZC) -> Self {
        let (tx_power_change, rx_power_change): (
            Sender<DevicesDimmerManagerNotification>,
            Receiver<DevicesDimmerManagerNotification>,
        ) = mpsc::channel();

        Self {
            devices: vec![],
            zero_crossing_pin,
            tx_power_change,
            rx_power_change,
            counter: 1,
        }
    }

    pub fn wait_zero_crossing(&mut self) -> Result<(), RbdDimmerError> {
        if self.read_power_update_message().is_err() {
            return Err(RbdDimmerError::from(
                RbdDimmerErrorKind::ChannelCommunicationDisconnected,
            ));
        }

        let result = self.zero_crossing_pin.wait_for_rising_edge();

        self.call_all_dimmer(self.counter);

        self.counter += 1;

        if self.counter > 100 {
            self.counter = 1;
        }

        result
    }

    pub fn sender(&mut self) -> Sender<DevicesDimmerManagerNotification> {
        self.tx_power_change.clone()
    }

    pub fn add(&mut self, device: DimmerDevice<O>) {
        self.devices.push(device);
    }

    // For each message in channel.
    // We update dimmer until channel is empty.
    // If channel is close, exit.
    fn read_power_update_message(&mut self) -> Result<(), TryRecvError> {
        loop {
            match self.rx_power_change.try_recv() {
                Ok(data) => self.update_dimmer_power(data),
                Err(TryRecvError::Disconnected) => return Err(TryRecvError::Disconnected),
                Err(TryRecvError::Empty) => break,
            }
        }

        Ok(())
    }

    // Update one dimmer power
    fn update_dimmer_power(&mut self, data: DevicesDimmerManagerNotification) {
        match self.devices.iter_mut().find(|d| d.id == data.id) {
            None => {}
            Some(device) => device.set_power(data.power),
        }
    }

    // Call all dimmer with tick
    fn call_all_dimmer(&mut self, counter: u8) {
        for dimmer in self.devices.iter_mut() {
            // TODO ignore error?
            let _ = dimmer.tick(counter);
        }
    }

    // TODO remove()?
    // TODO stop()?
}
