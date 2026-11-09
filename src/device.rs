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
    /// Firmware major.minor.patch from block 0, read once per connection.
    pub firmware: Option<[u8; 3]>,
}

impl Default for Blocks {
    fn default() -> Self {
        Self {
            blend: [0; 6],
            settings: [0; 38],
            headphones: [0; 2],
            firmware: None,
        }
    }
}

/// First firmware that runs the four-band EQ on the unit's own DSP.
pub const EQ_FIRMWARE: [u8; 3] = [1, 0, 15];
pub const EQ_BANDS: usize = 4;
const EQ_SWITCH: (usize, u8) = (2, 0x01);
const EQ_FIRST_BAND: usize = 18;
const EQ_BAND_STRIDE: usize = 5;
const EQ_BAND_ON: u8 = 0x01;
const EQ_TYPE_MASK: u8 = 0x0e;
const EQ_MAX_TYPE: u8 = 5;
const EQ_FREQ: (u16, u16) = (20, 20000);
const EQ_GAIN_QUARTERS: i8 = 48;
const EQ_Q_BYTE: i8 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BandField {
    On,
    Type,
    Freq,
    Gain,
    Q,
}

impl BandField {
    const ALL: [(BandField, &'static str); 5] = [
        (BandField::On, "On"),
        (BandField::Type, "Type"),
        (BandField::Freq, "Freq"),
        (BandField::Gain, "Gain"),
        (BandField::Q, "Q"),
    ];
}

/// One validated command, independent of the device's current bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Flag {
        block: u16,
        offset: usize,
        mask: u8,
        on: bool,
    },
    Byte {
        block: u16,
        offset: usize,
        value: u8,
    },
    Band {
        band: usize,
        field: BandField,
        value: i32,
    },
}

fn band_offset(band: usize) -> usize {
    EQ_FIRST_BAND + band * EQ_BAND_STRIDE
}

fn band_key(key: &str) -> Option<(usize, BandField)> {
    let rest = key.strip_prefix("eq")?;
    let digit = rest.chars().next()?.to_digit(10)? as usize;
    if !(1..=EQ_BANDS).contains(&digit) {
        return None;
    }
    let name = &rest[1..];
    BandField::ALL
        .iter()
        .find(|(_, suffix)| *suffix == name)
        .map(|(field, _)| (digit - 1, *field))
}

fn band_valid(settings: &[u8; 38], band: usize) -> bool {
    let at = band_offset(band);
    let kind = (settings[at] & EQ_TYPE_MASK) >> 1;
    let gain = settings[at + 1] as i8;
    let freq = u16::from_le_bytes([settings[at + 2], settings[at + 3]]);
    let q = settings[at + 4] as i8;
    kind <= EQ_MAX_TYPE
        && (EQ_FREQ.0..=EQ_FREQ.1).contains(&freq)
        && (-EQ_GAIN_QUARTERS..=EQ_GAIN_QUARTERS).contains(&gain)
        && (-EQ_Q_BYTE..=EQ_Q_BYTE).contains(&q)
}

/// Whether the unit runs the EQ: firmware 1.0.15 or later, and every band
/// holds values in Wave Link's ranges. Anything else leaves the EQ untouched.
pub fn eq_status(blocks: &Blocks) -> Result<(), &'static str> {
    match blocks.firmware {
        None => Err("firmwareUnknown"),
        Some(version) if version < EQ_FIRMWARE => Err("firmwareOld"),
        _ if !(0..EQ_BANDS).all(|band| band_valid(&blocks.settings, band)) => {
            Err("unsupportedBlock")
        }
        _ => Ok(()),
    }
}

/// Q is stored as round(64 log10 Q); report it to three significant digits,
/// which is enough for every byte to round-trip exactly.
fn q_value(byte: i8) -> f64 {
    let q = 10f64.powf(f64::from(byte) / 64.0);
    let scale = 10f64.powi(2 - q.log10().floor() as i32);
    (q * scale).round() / scale
}

fn q_byte(q: f64) -> Option<i8> {
    if q <= 0.0 {
        return None;
    }
    let byte = (64.0 * q.log10()).round();
    (-64.0..=64.0).contains(&byte).then_some(byte as i8)
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
    let mut state = json!({
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
    });
    if let Some([major, minor, patch]) = blocks.firmware {
        state["firmware"] = format!("{major}.{minor}.{patch}").into();
    }
    match eq_status(blocks) {
        Ok(()) => {
            let s = &blocks.settings;
            state["eq"] = (s[EQ_SWITCH.0] & EQ_SWITCH.1 != 0).into();
            for band in 0..EQ_BANDS {
                let at = band_offset(band);
                let n = band + 1;
                state[format!("eq{n}On")] = (s[at] & EQ_BAND_ON != 0).into();
                state[format!("eq{n}Type")] = ((s[at] & EQ_TYPE_MASK) >> 1).into();
                state[format!("eq{n}Gain")] = (f64::from(s[at + 1] as i8) / 4.0).into();
                state[format!("eq{n}Freq")] = u16::from_le_bytes([s[at + 2], s[at + 3]]).into();
                state[format!("eq{n}Q")] = q_value(s[at + 4] as i8).into();
            }
        }
        Err(reason) => state["eqUnavailable"] = reason.into(),
    }
    Ok(state)
}

fn binary(value: f64) -> Result<bool, String> {
    if value != 0.0 && value != 1.0 {
        return Err("Expected 0 or 1".into());
    }
    Ok(value != 0.0)
}

fn in_range(value: f64, low: f64, high: f64) -> Result<f64, String> {
    if !(low..=high).contains(&value) {
        return Err("Value out of range".into());
    }
    Ok(value)
}

/// Validate a command's key and value without touching hardware.
fn parse(key: &str, value: f64) -> Result<Edit, String> {
    if !value.is_finite() {
        return Err("Control value must be a finite number".into());
    }
    let flag = match key {
        "mute" => Some((4, 1, 1, false)),
        "phantom" => Some((4, 1, 2, false)),
        "lowCut" => Some((4, 1, 16, false)),
        "expander" => Some((4, 1, 32, false)),
        "voiceTune" => Some((4, 1, 64, false)),
        "compressor" => Some((4, 1, 128, false)),
        "clipguard" => Some((4, 2, 4, true)),
        "lowImpedance" => Some((5, 1, 2, false)),
        "eq" => Some((4, EQ_SWITCH.0, EQ_SWITCH.1, false)),
        _ => None,
    };
    if let Some((block, offset, mask, inverted)) = flag {
        return Ok(Edit::Flag {
            block,
            offset,
            mask,
            on: binary(value)? != inverted,
        });
    }
    if let Some((band, field)) = band_key(key) {
        let raw = match field {
            BandField::On => i32::from(binary(value)?),
            BandField::Type => {
                if value.fract() != 0.0 {
                    return Err("Expected a filter type from 0 to 5".into());
                }
                in_range(value, 0.0, f64::from(EQ_MAX_TYPE))? as i32
            }
            BandField::Freq => {
                in_range(value, f64::from(EQ_FREQ.0), f64::from(EQ_FREQ.1))?.round() as i32
            }
            BandField::Gain => (in_range(value, -12.0, 12.0)? * 4.0).round_ties_even() as i32,
            BandField::Q => i32::from(q_byte(value).ok_or("Value out of range")?),
        };
        return Ok(Edit::Band {
            band,
            field,
            value: raw,
        });
    }
    let (block, offset, low, high) = match key {
        "gain" => (4, 0, 0.0, 80.0),
        "headphones" => (5, 0, -60.0, 0.0),
        "balance" => (1, 0, 0.0, 200.0),
        "strength" => (4, 10, 0.0, 100.0),
        _ => return Err("Unknown control".into()),
    };
    in_range(value, low, high)?;
    // Integer controls truncate fractions, and headphone quarter steps use
    // ties to even, matching the original command protocol's conversion.
    let byte = if key == "headphones" {
        (-value * 4.0).round_ties_even() as u8
    } else {
        value as u8
    };
    Ok(Edit::Byte {
        block,
        offset,
        value: byte,
    })
}

/// Reject unknown controls and out-of-range values before any USB transfer.
pub fn check_command(key: &str, value: f64) -> Result<(), String> {
    parse(key, value).map(|_| ())
}

fn block_bytes(blocks: &Blocks, block: u16) -> Vec<u8> {
    match block {
        1 => blocks.blend.to_vec(),
        4 => blocks.settings.to_vec(),
        5 => blocks.headphones.to_vec(),
        _ => unreachable!("validated block"),
    }
}

/// Return only the block being changed; all other bytes and bits are preserved.
pub fn modify(blocks: &Blocks, key: &str, value: f64) -> Result<(u16, Vec<u8>), String> {
    let edit = parse(key, value)?;
    validate(blocks)?;
    match edit {
        Edit::Flag {
            block,
            offset,
            mask,
            on,
        } => {
            if key == "eq" {
                eq_status(blocks).map_err(eq_unavailable)?;
            }
            let mut data = block_bytes(blocks, block);
            if on {
                data[offset] |= mask;
            } else {
                data[offset] &= !mask;
            }
            Ok((block, data))
        }
        Edit::Byte {
            block,
            offset,
            value,
        } => {
            let mut data = block_bytes(blocks, block);
            data[offset] = value;
            Ok((block, data))
        }
        Edit::Band { band, field, value } => {
            eq_status(blocks).map_err(eq_unavailable)?;
            let mut data = blocks.settings.to_vec();
            let at = band_offset(band);
            match field {
                BandField::On => {
                    data[at] = (data[at] & !EQ_BAND_ON) | value as u8;
                }
                BandField::Type => {
                    data[at] = (data[at] & !EQ_TYPE_MASK) | ((value as u8) << 1);
                }
                BandField::Gain => data[at + 1] = value as i8 as u8,
                BandField::Freq => {
                    data[at + 2..at + 4].copy_from_slice(&(value as u16).to_le_bytes());
                }
                BandField::Q => data[at + 4] = value as i8 as u8,
            }
            Ok((4, data))
        }
    }
}

/// Whether a control belongs to the hardware EQ.
pub fn is_eq_control(key: &str) -> bool {
    key == "eq" || band_key(key).is_some()
}

pub fn eq_unavailable(reason: &str) -> String {
    match reason {
        "firmwareOld" => "Hardware EQ needs Wave XLR MK.2 firmware 1.0.15 or later".into(),
        "unsupportedBlock" => "Device EQ settings are unrecognized; EQ left untouched".into(),
        _ => "Firmware version unknown; hardware EQ unavailable".into(),
    }
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
    firmware: Option<[u8; 3]>,
}

/// Block 0 (117 bytes as Wave Link requests it) holds the firmware version at
/// bytes 5 to 7. A short or failed read only disables the hardware EQ.
fn read_firmware(handle: &DeviceHandle<Context>) -> Option<[u8; 3]> {
    let mut identity = [0u8; 117];
    let count = handle
        .read_control(0xc1, REQUEST, 0, CONTROL_INDEX, &mut identity, TIMEOUT)
        .ok()?;
    (count >= 8).then(|| [identity[5], identity[6], identity[7]])
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
        let firmware = read_firmware(&handle);
        return Ok(Box::new(UsbDevice { handle, firmware }));
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
            firmware: self.firmware,
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
            firmware: None,
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

    /// Wave XLR MK.2 on 1.0.15 with band 3 as captured from Wave Link:
    /// `05 30 bf 02 f2` is enabled, bell, +12 dB, 703 Hz, Q 0.60.
    fn eq_fixture() -> Blocks {
        let mut blocks = fixture();
        blocks.firmware = Some([1, 0, 15]);
        blocks.settings[2] = 0xa1; // EQ on, ClipGuard on, unknown bits set
        let bands: [[u8; 5]; 4] = [
            [0xf2, 0x00, 60, 0, 0xf6],      // high pass, unknown high bits set
            [0x22, 0xfc, 0xe3, 0x00, 0x00], // low shelf off, -1 dB, 227 Hz
            [0x05, 0x30, 0xbf, 0x02, 0xf2], // bell on, +12 dB, 703 Hz
            [0x09, 0xd0, 0xfd, 0x27, 0x40], // high shelf on, -12 dB, 10237 Hz, Q 10
        ];
        for (band, bytes) in bands.iter().enumerate() {
            let at = 18 + band * 5;
            blocks.settings[at..at + 5].copy_from_slice(bytes);
        }
        blocks
    }

    #[test]
    fn eq_decodes_wave_link_bands() {
        let state = decode(&eq_fixture()).unwrap();
        assert_eq!(state["firmware"], "1.0.15");
        assert_eq!(state["eq"], true);
        assert_eq!(state["clipguard"], true);
        assert!(state.get("eqUnavailable").is_none());
        assert_eq!(state["eq1Type"], 1); // 0xf2 & 0x0e = 2: type 1, low shelf
        assert_eq!(state["eq1On"], false);
        assert_eq!(state["eq1Freq"], 60);
        assert_eq!(state["eq1Q"], 0.698);
        assert_eq!(state["eq2Gain"], -1.0);
        assert_eq!(state["eq2Freq"], 227);
        assert_eq!(state["eq3On"], true);
        assert_eq!(state["eq3Type"], 2);
        assert_eq!(state["eq3Gain"], 12.0);
        assert_eq!(state["eq3Freq"], 703);
        assert_eq!(state["eq3Q"], 0.604);
        assert_eq!(state["eq4Type"], 4);
        assert_eq!(state["eq4Gain"], -12.0);
        assert_eq!(state["eq4Freq"], 10237);
        assert_eq!(state["eq4Q"], 10.0);
    }

    #[test]
    fn eq_requires_known_firmware_and_valid_bands() {
        let mut blocks = eq_fixture();
        for (firmware, reason) in [
            (None, "firmwareUnknown"),
            (Some([1, 0, 14]), "firmwareOld"),
            (Some([0, 9, 99]), "firmwareOld"),
        ] {
            blocks.firmware = firmware;
            let state = decode(&blocks).unwrap();
            assert_eq!(state["eqUnavailable"], reason);
            assert!(state.get("eq").is_none() && state.get("eq1Freq").is_none());
            assert!(modify(&blocks, "eq", 0.0).is_err());
            assert!(modify(&blocks, "eq3Gain", 0.0).is_err());
            // Other controls still work.
            assert!(modify(&blocks, "mute", 1.0).is_ok());
        }
        for firmware in [[1, 0, 16], [1, 1, 0], [2, 0, 0]] {
            blocks.firmware = Some(firmware);
            assert_eq!(decode(&blocks).unwrap()["eq"], true);
        }
        for corrupt in [
            (18 + 2, 0x00),     // band 1 frequency 0 Hz
            (18 + 1, 0x31),     // band 1 gain +12.25 dB
            (18 + 5 + 4, 0x41), // band 2 Q byte 65
            (18 + 10, 0x0c),    // band 3 type 6
        ] {
            let mut blocks = eq_fixture();
            blocks.settings[corrupt.0] = corrupt.1;
            let state = decode(&blocks).unwrap();
            assert_eq!(state["eqUnavailable"], "unsupportedBlock", "{corrupt:?}");
            assert!(modify(&blocks, "eq2Freq", 100.0).is_err());
            assert_eq!(state["gain"], 43);
        }
    }

    #[test]
    fn every_eq_field_preserves_all_other_bits_and_bytes() {
        let blocks = eq_fixture();
        for band in 1..=4 {
            let at = 18 + (band - 1) * 5;
            for (field, value, offsets) in [
                ("On", 1.0, vec![at]),
                ("On", 0.0, vec![at]),
                ("Type", 5.0, vec![at]),
                ("Type", 0.0, vec![at]),
                ("Freq", 20000.0, vec![at + 2, at + 3]),
                ("Freq", 20.0, vec![at + 2, at + 3]),
                ("Gain", -11.75, vec![at + 1]),
                ("Gain", 12.0, vec![at + 1]),
                ("Q", 0.1, vec![at + 4]),
                ("Q", 10.0, vec![at + 4]),
            ] {
                let key = format!("eq{band}{field}");
                let (block, data) = modify(&blocks, &key, value).unwrap();
                assert_eq!(block, 4);
                let after = replaced(&blocks, block, &data);
                let decoded = decode(&after).unwrap();
                let read = match &decoded[&key] {
                    Value::Bool(on) => Some(f64::from(u8::from(*on))),
                    other => other.as_f64(),
                };
                assert_eq!(read, Some(value), "{key}");
                for (i, (new, old)) in data.iter().zip(&blocks.settings).enumerate() {
                    if !offsets.contains(&i) {
                        assert_eq!(new, old, "{key} touched byte {i}");
                    }
                }
                if field == "On" {
                    assert_eq!(data[at] & !1, blocks.settings[at] & !1);
                }
                if field == "Type" {
                    assert_eq!(data[at] & !0x0e, blocks.settings[at] & !0x0e);
                }
            }
        }
        for value in [0.0, 1.0] {
            let (_, data) = modify(&blocks, "eq", value).unwrap();
            assert_eq!(data[2] & !1, blocks.settings[2] & !1);
            assert_eq!(
                decode(&replaced(&blocks, 4, &data)).unwrap()["eq"],
                value == 1.0
            );
            let mut rest = data.clone();
            rest[2] = blocks.settings[2];
            assert_eq!(rest, blocks.settings.to_vec());
        }
    }

    #[test]
    fn eq_values_round_trip_and_invalid_values_fail_before_hardware() {
        let blocks = eq_fixture();
        for byte in -64i8..=64 {
            let q = q_value(byte);
            assert_eq!(q_byte(q), Some(byte), "Q byte {byte} via {q}");
            let (_, data) = modify(&blocks, "eq2Q", q).unwrap();
            assert_eq!(data[18 + 5 + 4] as i8, byte);
        }
        for quarters in -48..=48 {
            let gain = f64::from(quarters) / 4.0;
            let (_, data) = modify(&blocks, "eq1Gain", gain).unwrap();
            assert_eq!(data[19] as i8, quarters as i8);
        }
        let (_, data) = modify(&blocks, "eq1Freq", 1000.4).unwrap();
        assert_eq!(u16::from_le_bytes([data[20], data[21]]), 1000);
        for (key, value) in [
            ("eq1Freq", 19.0),
            ("eq1Freq", 20001.0),
            ("eq1Gain", 12.5),
            ("eq1Gain", -12.25),
            ("eq1Q", 0.09),
            ("eq1Q", 10.6),
            ("eq1Q", 0.0),
            ("eq1Q", -1.0),
            ("eq1Type", 6.0),
            ("eq1Type", 1.5),
            ("eq1Type", -1.0),
            ("eq1On", 0.5),
            ("eq", 2.0),
            ("eq1Gain", f64::NAN),
            ("eq0Freq", 100.0),
            ("eq5Freq", 100.0),
            ("eq1Bogus", 1.0),
            ("eq12Freq", 100.0),
        ] {
            assert!(check_command(key, value).is_err(), "{key} {value}");
            assert!(modify(&blocks, key, value).is_err(), "{key} {value}");
        }
    }
}
