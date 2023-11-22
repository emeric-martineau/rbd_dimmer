# Rust crate for RBDDimmer

## What is RBDDimmer ?

RBDDimmer of [RoboDyn](https://robotdyn.fr.aliexpress.com) is device build around two triac.

![RbdDimmer](doc/rbddimmer.jpg)

You can look [official source](https://github.com/RobotDynOfficial/RBDDimmer) code on Github.

This crate not works like official library. Power is turn on/off on Zero Crossing event if device has MOC3021 triac to limit power-lost.

## Schema

![RbdDimmer schema](doc/rbddimmer_schema.jpg)

## Test hardware

This crate has only tested on ESP32-WROOM-32 microcontroler.

## Example

First, you need provide implementation of some trait (hardware abstract):
```rust
use rbd_dimmer::*;

struct MyAbstractPinForDimmer {
    // Some necessary fields
}

impl OutputPin for MyAbstractPinForDimmer {
    fn set_high(&mut self) -> Result<(), RbdDimmerError> {
        // Do something
    }

    fn set_low(&mut self) -> Result<(), RbdDimmerError> {
        // Do something
    }
}

struct MyAbstractZcPin {
    // Some necessary fields
}

impl ZeroCrossongPin for MyAbstractZcPin {
    fn wait_for_rising_edge(&mut self) -> Result<(), RbdDimmerError> {
        // Do something
    }
}
```

Then, you need create thread that call `wait_zero_crossing()` method:
```rust
let my_pin = MyAbstractPinForDimmer::new();
let dim_device = DimmerDevice::new(0, my_pin);
let zero_crossing_pin = MyAbstractZcPin::new();

let mut devices_dimmer_manager: DevicesDimmerManager<MyAbstractPinForDimmer, MyAbstractZcPin> = DevicesDimmerManager::new(zero_crossing_pin);

// Get channel to change power of device from another thread
let sender_power = devices_dimmer_manager.sender();

// Add the device
devices_dimmer_manager.add(dim_device);

// Loop for ever
thread::spawn(|| {
    loop {
        let _ = devices_dimmer_manager.wait_zero_crossing();
    }
});
```

If you want change power value of device:
```rust
sender_power.send(DevicesDimmerManagerNotification {
    id: 0,
    power: 10
}).unwrap();
```

## License

The code is released under MIT License to allow every body to use it in all conditions. If you love open-source software and this crate, please give some money to [HaikuOS](https://haiku-os.org/) or [ReactOS](https://reactos.org).
