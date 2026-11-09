//! Device-side change notifications through the kernel's ALSA control device.
//!
//! The Wave XLR MK.2 reports gain, mute and headphone changes (knob, button or
//! vendor writes) on its audio-control interrupt endpoint, which snd-usb-audio
//! turns into control events. Subscribing lets the worker sleep until the
//! device changes instead of polling for it. This uses the raw kernel ioctl and
//! `read`, without libasound, and never opens the USB device.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::{AsRawFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
};

/// `_IOWR('U', 0x16, int)`: SNDRV_CTL_IOCTL_SUBSCRIBE_EVENTS.
const SUBSCRIBE_EVENTS: libc::c_ulong = 0xc004_5516;
/// `sizeof(struct snd_ctl_event)`: int type, then a union whose element arm is
/// a mask and a 64-byte `snd_ctl_elem_id` (verified against <sound/asound.h>).
const EVENT_SIZE: usize = 72;
/// `offsetof(struct snd_ctl_event, data.elem.id.name)`, 44 bytes long.
const NAME: std::ops::Range<usize> = 24..68;
const EVENT_ELEM: u32 = 0;
const MASK_VALUE: u32 = 1;
const MASK_REMOVE: u32 = !0;

/// Vendor blocks a changed control lives in, as a bit set (see `block_bits`).
pub const BLEND: u8 = 1;
pub const SETTINGS: u8 = 2;
pub const HEADPHONES: u8 = 4;
pub const ALL_BLOCKS: u8 = BLEND | SETTINGS | HEADPHONES;

pub enum Drained {
    /// Control values changed in these vendor blocks.
    Changed(u8),
    /// Nothing relevant (or nothing at all) was pending.
    Quiet,
    /// The card is gone or the descriptor failed; drop this watcher.
    Closed,
}

pub struct ControlEvents {
    file: File,
}

impl ControlEvents {
    /// Subscribe to the attached MK.2's control events, if its card exists.
    pub fn open() -> Option<Self> {
        let path = find_control(Path::new("/sys/class/sound"))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(Path::new("/dev/snd").join(path.file_name()?))
            .ok()?;
        let mut subscribe: libc::c_int = 1;
        // SAFETY: valid open descriptor and a pointer to a live int, as the ioctl expects.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), SUBSCRIBE_EVENTS, &mut subscribe) };
        (result == 0).then_some(Self { file })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Read every pending event without blocking.
    pub fn drain(&mut self) -> Drained {
        let mut buffer = [0u8; EVENT_SIZE * 16];
        let mut changed = 0;
        loop {
            match self.file.read(&mut buffer) {
                Ok(0) => return Drained::Closed,
                Ok(count) => {
                    for event in buffer[..count].as_chunks::<EVENT_SIZE>().0 {
                        if value_changed(event) {
                            changed |= block_bits(&event[NAME]);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Drained::Closed,
            }
        }
        if changed != 0 {
            Drained::Changed(changed)
        } else {
            Drained::Quiet
        }
    }
}

fn value_changed(event: &[u8; EVENT_SIZE]) -> bool {
    let word = |at: usize| u32::from_ne_bytes(event[at..at + 4].try_into().unwrap());
    let (kind, mask) = (word(0), word(4));
    kind == EVENT_ELEM && mask != MASK_REMOVE && mask & MASK_VALUE != 0
}

/// Capture volume/switch are gain and mute (settings block); playback volume
/// is the headphone level. Anything else re-reads every block.
fn block_bits(name: &[u8]) -> u8 {
    let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
    let has = |needle: &[u8]| name.windows(needle.len()).any(|w| w == needle);
    if has(b"Capture") {
        SETTINGS
    } else if has(b"Playback") {
        HEADPHONES
    } else {
        ALL_BLOCKS
    }
}

fn read_hex(path: &Path) -> Option<u16> {
    u16::from_str_radix(fs::read_to_string(path).ok()?.trim(), 16).ok()
}

/// The `controlC*` class entry whose sound card belongs to a 0fd9:00b6 device.
/// `controlCn/device` is the card; the card's `device` is the USB interface,
/// whose parent directory is the USB device with the identity files.
fn find_control(class: &Path) -> Option<PathBuf> {
    fs::read_dir(class).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = entry.file_name();
        if !name.to_str()?.starts_with("controlC") {
            return None;
        }
        let interface = fs::canonicalize(path.join("device/device")).ok()?;
        let usb = interface.parent()?;
        (read_hex(&usb.join("idVendor")) == Some(0x0fd9)
            && read_hex(&usb.join("idProduct")) == Some(0x00b6))
        .then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn event(kind: u32, mask: u32) -> [u8; EVENT_SIZE] {
        let mut bytes = [0u8; EVENT_SIZE];
        bytes[..4].copy_from_slice(&kind.to_ne_bytes());
        bytes[4..8].copy_from_slice(&mask.to_ne_bytes());
        bytes
    }

    fn named(name: &str) -> [u8; EVENT_SIZE] {
        let mut bytes = event(EVENT_ELEM, MASK_VALUE);
        bytes[NAME.start..NAME.start + name.len()].copy_from_slice(name.as_bytes());
        bytes
    }

    #[test]
    fn control_names_map_to_vendor_blocks() {
        let bits = |name| block_bits(&named(name)[NAME]);
        assert_eq!(bits("Elgato Wave XLR MK.2 Capture Volume"), SETTINGS);
        assert_eq!(bits("Elgato Wave XLR MK.2 Capture Switch"), SETTINGS);
        assert_eq!(bits("Elgato Wave XLR MK.2 Playback Volume"), HEADPHONES);
        assert_eq!(bits("Internal Validity"), ALL_BLOCKS);
        assert_eq!(bits(""), ALL_BLOCKS);
        // A full 44-byte name without a terminator is still bounded.
        assert_eq!(block_bits(&[b'x'; 44]), ALL_BLOCKS);
    }

    #[test]
    fn only_value_changes_count() {
        assert!(value_changed(&event(EVENT_ELEM, MASK_VALUE)));
        assert!(value_changed(&event(EVENT_ELEM, MASK_VALUE | 2)));
        assert!(!value_changed(&event(EVENT_ELEM, 2))); // info only
        assert!(!value_changed(&event(EVENT_ELEM, 4))); // element added
        assert!(!value_changed(&event(EVENT_ELEM, MASK_REMOVE)));
        assert!(!value_changed(&event(1, MASK_VALUE))); // not an element event
    }

    #[test]
    fn finds_only_the_mk2_card() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "wave-control-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let add = |card: &str, vendor: &str, product: &str| {
            let usb = root.join("devices").join(card);
            let interface = usb.join(format!("{card}:1.0"));
            let sound = interface.join("sound").join(card);
            fs::create_dir_all(&sound).unwrap();
            fs::write(usb.join("idVendor"), vendor).unwrap();
            fs::write(usb.join("idProduct"), product).unwrap();
            symlink(&interface, sound.join("device")).unwrap();
            let class = root.join("class");
            fs::create_dir_all(class.join(format!("controlC{card}"))).unwrap();
            symlink(&sound, class.join(format!("controlC{card}/device"))).unwrap();
        };
        add("0", "0fd9\n", "007d\n"); // original Wave XLR
        add("1", "1234\n", "00b6\n");
        assert_eq!(find_control(&root.join("class")), None);
        add("2", "0fd9\n", "00b6\n");
        assert_eq!(
            find_control(&root.join("class")),
            Some(root.join("class/controlC2"))
        );
        fs::create_dir_all(root.join("class/pcmC2D0p")).unwrap();
        assert!(find_control(&root.join("missing")).is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
