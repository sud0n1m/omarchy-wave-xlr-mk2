//! Wave XLR MK.2 vendor controls. Protocol constants are documented in README.
//!
//! Audio streaming remains owned by the kernel: only vendor interface 3 is
//! claimed, without changing configuration or detaching a kernel driver.

use rusb::{Context, DeviceHandle, UsbContext};
use serde_json::{Value, json};
use std::time::Duration;

const VENDOR_ID: u16 = 0x0fd9;
const PRODUCT_ID: u16 = 0x00b6;
const CONTROL_INTERFACE: u8 = 3;
const CONTROL_INDEX: u16 = 0x0203;
const REQUEST: u8 = 1;
const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocks {
    pub blend: [u8; 6],
    pub settings: [u8; 38],
    pub headphones: [u8; 2],
}

impl Default for Blocks {
    fn default() -> Self {
        Self {
            blend: [0; 6],
            settings: [0; 38],
            headphones: [0; 2],
        }
    }
}

fn validate(blocks: &Blocks) -> Result<(), String> {
    if blocks.settings[0] > 80
        || blocks.headphones[0] > 240
        || blocks.blend[0] > 200
        || blocks.settings[10] > 100
    {
        return Err("Unexpected device state; controls disabled".into());
    }
    Ok(())
}

pub fn decode(blocks: &Blocks) -> Result<Value, String> {
    validate(blocks)?;
    let flags = blocks.settings[1];
    Ok(json!({
        "connected": true,
        "usb": true,
        "gain": blocks.settings[0],
        "headphones": -f64::from(blocks.headphones[0]) / 4.0,
        "balance": blocks.blend[0],
        "strength": blocks.settings[10],
        "mute": flags & 1 != 0,
        "phantom": flags & 2 != 0,
        "lowCut": flags & 16 != 0,
        "expander": flags & 32 != 0,
        "voiceTune": flags & 64 != 0,
        "compressor": flags & 128 != 0,
        "clipguard": blocks.settings[2] & 4 == 0,
        "lowImpedance": blocks.headphones[1] & 2 != 0,
    }))
}

/// Return only the block being changed; all other bytes and bits are preserved.
/// Integer controls truncate fractions, and headphone quarter steps use ties to
/// even, matching the original command protocol's numeric conversion.
pub fn modify(blocks: &Blocks, key: &str, value: f64) -> Result<(u16, Vec<u8>), String> {
    if !value.is_finite() {
        return Err("Control value must be a finite number".into());
    }
    validate(blocks)?;
    let flag = match key {
        "mute" => Some((4, 1, 1, false)),
        "phantom" => Some((4, 1, 2, false)),
        "lowCut" => Some((4, 1, 16, false)),
        "expander" => Some((4, 1, 32, false)),
        "voiceTune" => Some((4, 1, 64, false)),
        "compressor" => Some((4, 1, 128, false)),
        "clipguard" => Some((4, 2, 4, true)),
        "lowImpedance" => Some((5, 1, 2, false)),
        _ => None,
    };
    if let Some((block, offset, mask, inverted)) = flag {
        if value != 0.0 && value != 1.0 {
            return Err("Expected 0 or 1".into());
        }
        let mut data = if block == 4 {
            blocks.settings.to_vec()
        } else {
            blocks.headphones.to_vec()
        };
        if (value != 0.0) != inverted {
            data[offset] |= mask;
        } else {
            data[offset] &= !mask;
        }
        return Ok((block, data));
    }

    let (block, offset, low, high) = match key {
        "gain" => (4, 0, 0.0, 80.0),
        "headphones" => (5, 0, -60.0, 0.0),
        "balance" => (1, 0, 0.0, 200.0),
        "strength" => (4, 10, 0.0, 100.0),
        _ => return Err("Unknown control".into()),
    };
    if !(low..=high).contains(&value) {
        return Err("Value out of range".into());
    }
    let mut data = match block {
        1 => blocks.blend.to_vec(),
        4 => blocks.settings.to_vec(),
        5 => blocks.headphones.to_vec(),
        _ => unreachable!(),
    };
    data[offset] = if key == "headphones" {
        (-value * 4.0).round_ties_even() as u8
    } else {
        value as u8
    };
    Ok((block, data))
}

pub trait Device {
    fn read(&mut self) -> Result<Blocks, String>;
    fn write(&mut self, block: u16, data: &[u8]) -> Result<(), String>;
}

#[derive(Debug)]
pub struct OpenError {
    pub message: String,
    /// True only for Access on a positively identified MK.2's open or claim.
    pub permission: bool,
}

fn open_error(operation: &str, error: rusb::Error, matched: bool) -> OpenError {
    OpenError {
        message: format!("{operation}: {error}"),
        permission: matched && error == rusb::Error::Access,
    }
}

struct UsbDevice {
    // rusb's handle retains its Context and releases all claimed interfaces
    // before closing. RAII also handles failure after open but before claim.
    handle: DeviceHandle<Context>,
}

pub fn open() -> Result<Box<dyn Device>, OpenError> {
    let context =
        Context::new().map_err(|error| open_error("USB initialization failed", error, false))?;
    let devices = context
        .devices()
        .map_err(|error| open_error("USB enumeration failed", error, false))?;
    for device in devices.iter() {
        let descriptor = match device.device_descriptor() {
            Ok(descriptor) => descriptor,
            Err(_) => continue,
        };
        if descriptor.vendor_id() != VENDOR_ID || descriptor.product_id() != PRODUCT_ID {
            continue;
        }
        let handle = device
            .open()
            .map_err(|error| open_error("Cannot open Wave XLR MK.2", error, true))?;
        // Auto detach is off by default. Do not enable it: a busy interface
        // belongs to another owner, and must never trigger ALSA fallback.
        handle
            .claim_interface(CONTROL_INTERFACE)
            .map_err(|error| open_error("USB control interface unavailable", error, true))?;
        return Ok(Box::new(UsbDevice { handle }));
    }
    Err(OpenError {
        message: "Wave XLR MK.2 disconnected".into(),
        permission: false,
    })
}

fn check_length(block: u16, actual: usize, expected: usize) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "USB block {block}: transferred {actual}, expected {expected}; no further writes"
        ));
    }
    Ok(())
}

fn write_length(block: u16, data: &[u8]) -> Result<(), String> {
    let expected = match block {
        1 => 6,
        4 => 38,
        5 => 2,
        _ => return Err("Unknown USB control block".into()),
    };
    check_length(block, data.len(), expected)
}

impl UsbDevice {
    fn read_block<const N: usize>(&self, block: u16) -> Result<[u8; N], String> {
        let mut data = [0; N];
        let count = self
            .handle
            .read_control(0xc1, REQUEST, block, CONTROL_INDEX, &mut data, TIMEOUT)
            .map_err(|error| format!("USB block {block} read failed: {error}"))?;
        check_length(block, count, N)?;
        Ok(data)
    }
}

impl Device for UsbDevice {
    fn read(&mut self) -> Result<Blocks, String> {
        let blocks = Blocks {
            blend: self.read_block(1)?,
            settings: self.read_block(4)?,
            headphones: self.read_block(5)?,
        };
        validate(&blocks)?;
        Ok(blocks)
    }

    fn write(&mut self, block: u16, data: &[u8]) -> Result<(), String> {
        write_length(block, data)?;
        let count = self
            .handle
            .write_control(0x41, REQUEST, block, CONTROL_INDEX, data, TIMEOUT)
            .map_err(|error| format!("USB block {block} write failed: {error}"))?;
        check_length(block, count, data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Blocks {
        let mut blocks = Blocks {
            blend: [0xad; 6],
            settings: [0xad; 38],
            headphones: [0xad; 2],
        };
        blocks.blend[0] = 100;
        blocks.settings[0] = 43;
        blocks.settings[10] = 33;
        blocks.headphones[0] = 80;
        blocks
    }

    fn replaced(blocks: &Blocks, block: u16, data: &[u8]) -> Blocks {
        let mut after = blocks.clone();
        match block {
            1 => after.blend.copy_from_slice(data),
            4 => after.settings.copy_from_slice(data),
            5 => after.headphones.copy_from_slice(data),
            _ => panic!("unknown test block"),
        }
        after
    }

    #[test]
    fn every_flag_preserves_other_bits_and_bytes() {
        let blocks = fixture();
        for (key, expected_block, offset, mask) in [
            ("mute", 4, 1, 1),
            ("phantom", 4, 1, 2),
            ("lowCut", 4, 1, 16),
            ("expander", 4, 1, 32),
            ("voiceTune", 4, 1, 64),
            ("compressor", 4, 1, 128),
            ("clipguard", 4, 2, 4),
            ("lowImpedance", 5, 1, 2),
        ] {
            for value in [0.0, 1.0] {
                let (block, data) = modify(&blocks, key, value).unwrap();
                assert_eq!(block, expected_block);
                let after = replaced(&blocks, block, &data);
                assert_eq!(decode(&after).unwrap()[key], value != 0.0);
                let mut restored = data;
                let original: &[u8] = if block == 4 {
                    &blocks.settings
                } else {
                    &blocks.headphones
                };
                assert_eq!(restored[offset] & !mask, original[offset] & !mask);
                restored[offset] = original[offset];
                assert_eq!(restored, original);
            }
            for invalid in [-1.0, 0.5, 2.0, f64::NAN, f64::INFINITY] {
                assert!(modify(&blocks, key, invalid).is_err(), "{key}: {invalid}");
            }
        }
    }

    #[test]
    fn numeric_limits_units_and_unrelated_bytes_are_preserved() {
        let blocks = fixture();
        for (key, lo, hi, block_number, offset) in [
            ("gain", 0.0, 80.0, 4, 0),
            ("headphones", -60.0, 0.0, 5, 0),
            ("balance", 0.0, 200.0, 1, 0),
            ("strength", 0.0, 100.0, 4, 10),
        ] {
            for value in [lo, hi, (lo + hi) / 2.0] {
                let (block, data) = modify(&blocks, key, value).unwrap();
                assert_eq!(block, block_number);
                let mut after = replaced(&blocks, block, &data);
                assert_eq!(decode(&after).unwrap()[key].as_f64(), Some(value));
                match block {
                    1 => after.blend[offset] = blocks.blend[offset],
                    4 => after.settings[offset] = blocks.settings[offset],
                    5 => after.headphones[offset] = blocks.headphones[offset],
                    _ => unreachable!(),
                }
                assert_eq!(after, blocks);
            }
            for invalid in [lo - 0.001, hi + 0.001, f64::NAN, f64::NEG_INFINITY] {
                assert!(modify(&blocks, key, invalid).is_err(), "{key}: {invalid}");
            }
        }
    }

    #[test]
    fn quarter_step_rounding_matches_original_protocol() {
        let blocks = fixture();
        for (requested, expected) in [
            (-20.25, -20.25),
            (-20.125, -20.0),
            (-20.375, -20.5),
            (-59.875, -60.0),
        ] {
            let (block, data) = modify(&blocks, "headphones", requested).unwrap();
            assert_eq!(
                decode(&replaced(&blocks, block, &data)).unwrap()["headphones"],
                expected
            );
        }
        let (block, data) = modify(&blocks, "gain", 43.9).unwrap();
        assert_eq!(
            decode(&replaced(&blocks, block, &data)).unwrap()["gain"],
            43
        );
    }

    #[test]
    fn invalid_firmware_state_refuses_decode_and_modification() {
        for field in 0..4 {
            let mut blocks = fixture();
            match field {
                0 => blocks.settings[0] = 81,
                1 => blocks.headphones[0] = 241,
                2 => blocks.blend[0] = 201,
                _ => blocks.settings[10] = 101,
            }
            assert!(decode(&blocks).is_err());
            assert!(modify(&blocks, "mute", 1.0).is_err());
        }
        assert!(modify(&fixture(), "unknown", 0.0).is_err());
    }

    #[test]
    fn short_transfers_unknown_blocks_and_wrong_write_sizes_fail() {
        for (block, size) in [(1, 6), (4, 38), (5, 2)] {
            assert!(check_length(block, size, size).is_ok());
            assert!(write_length(block, &vec![0; size]).is_ok());
            for wrong in [0, size - 1, size + 1] {
                assert!(check_length(block, wrong, size).is_err());
                assert!(write_length(block, &vec![0; wrong]).is_err());
            }
        }
        assert!(write_length(2, &[0; 6]).is_err());
    }

    #[test]
    fn only_identified_access_errors_allow_alsa_fallback() {
        assert!(open_error("open", rusb::Error::Access, true).permission);
        assert!(!open_error("enumerate", rusb::Error::Access, false).permission);
        for error in [
            rusb::Error::Busy,
            rusb::Error::NoDevice,
            rusb::Error::Io,
            rusb::Error::NotFound,
        ] {
            assert!(!open_error("claim", error, true).permission);
        }
    }
}
