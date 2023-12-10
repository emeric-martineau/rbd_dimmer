use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use crate::zc::*;

#[derive(Debug, Clone, PartialEq, Copy)]
enum PinState {
    Low,
    High,
    Unknow,
}

struct FakePin {
    pin_state: PinState,
}

///////////////////////////////////////////////////////////////////////////////
// Good struct
impl FakePin {
    pub fn new() -> Self {
        Self {
            pin_state: PinState::Unknow,
        }
    }
}

impl OutputPin for FakePin {
    fn set_high(&mut self) -> Result<(), RbdDimmerError> {
        self.pin_state = PinState::High;
        Ok(())
    }

    fn set_low(&mut self) -> Result<(), RbdDimmerError> {
        self.pin_state = PinState::Low;
        Ok(())
    }
}

///////////////////////////////////////////////////////////////////////////////
// Fail struct
struct FakeFailPin;

impl FakeFailPin {
    pub fn new() -> Self {
        FakeFailPin {}
    }
}

impl OutputPin for FakeFailPin {
    fn set_high(&mut self) -> Result<(), RbdDimmerError> {
        Err(RbdDimmerError::from(RbdDimmerErrorKind::SetHigh))
    }

    fn set_low(&mut self) -> Result<(), RbdDimmerError> {
        Err(RbdDimmerError::from(RbdDimmerErrorKind::SetLow))
    }
}

#[test]
fn test_dimmer_device_pin_up_then_low() {
    let fake_pin = FakePin::new();
    let mut dim_device = DimmerDevice::new(0, fake_pin);

    dim_device.set_power(20);

    // Pin go high
    match dim_device.tick(10) {
        Ok(()) => assert_eq!(dim_device.pin().pin_state, PinState::High),
        Err(_) => panic!(),
    }

    // Pin go low
    match dim_device.tick(30) {
        Ok(()) => assert_eq!(dim_device.pin().pin_state, PinState::Low),
        Err(_) => panic!(),
    }
}

#[test]
fn test_dimmer_device_fail() {
    let fake_pin = FakeFailPin::new();
    let mut dim_device = DimmerDevice::new(0, fake_pin);

    dim_device.set_power(20);

    // Pin go high
    match dim_device.tick(10) {
        Ok(()) => panic!(),
        Err(e) => assert_eq!(e.kind, RbdDimmerErrorKind::SetHigh),
    }

    // Pin go low
    match dim_device.tick(30) {
        Ok(()) => panic!(),
        Err(e) => assert_eq!(e.kind, RbdDimmerErrorKind::SetLow),
    }
}

///////////////////////////////////////////////////////////////////////////////
// Test manager
struct FakeZeroCrossPin {
    tx_zc: Sender<bool>,
    rx_zc: Receiver<bool>,
}

impl ZeroCrossingPin for FakeZeroCrossPin {
    fn wait_for_rising_edge(&mut self) -> Result<(), RbdDimmerError> {
        self.rx_zc.recv().unwrap();
        Ok(())
    }
}

impl FakeZeroCrossPin {
    pub fn new() -> Self {
        let (tx_zc, rx_zc): (Sender<bool>, Receiver<bool>) = mpsc::channel();
        Self { tx_zc, rx_zc }
    }
}

#[test]
fn test_devices_dimmer_manager_turn_device_up_then_down() {
    let fake_pin = FakePin::new();

    let dim_device = DimmerDevice::new(0, fake_pin);
    let zero_crossing_pin = FakeZeroCrossPin::new();
    let zc_sender = zero_crossing_pin.tx_zc.clone();
    let mut devices_dimmer_manager: DevicesDimmerManager<FakePin, FakeZeroCrossPin> =
        DevicesDimmerManager::new(zero_crossing_pin);

    // Add the device
    devices_dimmer_manager.add(dim_device);

    // Set power to 10 of device 0
    let tx_power = devices_dimmer_manager.sender();
    tx_power
        .send(DevicesDimmerManagerNotification { id: 0, power: 10 })
        .unwrap();

    // Send a ZC signal
    zc_sender.send(true).unwrap();

    let result = devices_dimmer_manager.wait_zero_crossing();

    assert!(result.is_ok());

    assert_eq!(
        devices_dimmer_manager.devices[0].pin().pin_state,
        PinState::High
    );

    for _ in 1..11 {
        zc_sender.send(true).unwrap();

        let result = devices_dimmer_manager.wait_zero_crossing();

        assert!(result.is_ok());
    }

    assert_eq!(
        devices_dimmer_manager.devices[0].pin().pin_state,
        PinState::Low
    );
}
