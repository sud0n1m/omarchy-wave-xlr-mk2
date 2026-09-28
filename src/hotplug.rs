//! Passive USB presence notifications; no USB handles or control transfers.

use std::{
    fs, io,
    os::fd::{AsRawFd, RawFd},
    path::Path,
};

pub struct Hotplug {
    socket: udev::MonitorSocket,
}

impl Hotplug {
    /// Start listening before calling `attached`, so plug/unplug events during
    /// the initial inventory remain queued and cannot be missed.
    pub fn new() -> Result<Self, String> {
        let socket = udev::MonitorBuilder::new()
            .and_then(|builder| builder.match_subsystem_devtype("usb", "usb_device"))
            .and_then(|builder| builder.listen())
            .map_err(|error| format!("USB event monitor unavailable: {error}"))?;
        Ok(Self { socket })
    }

    pub fn fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    /// Drain the nonblocking socket. Reconcile once per batch of completed udev
    /// events, including removal events whose device attributes no longer exist.
    /// The kernel filter excludes USB interface and unrelated subsystem events.
    pub fn changed(&mut self) -> bool {
        self.socket.iter().count() != 0
    }
}

/// Read kernel-exported identity files once, at startup or after a USB event.
/// This does not initialize libusb, open a device node, or run a subprocess.
pub fn attached() -> Result<bool, String> {
    attached_in(Path::new("/sys/bus/usb/devices"))
        .map_err(|error| format!("USB presence inventory failed: {error}"))
}

fn read_id(path: &Path) -> io::Result<Option<u16>> {
    match fs::read_to_string(path) {
        Ok(text) => u16::from_str_radix(text.trim(), 16)
            .map(Some)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid USB identity")),
        // Interface entries lack these attributes. Disappearing files also occur
        // normally if a device is unplugged during this one-time inventory.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn attached_in(directory: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if read_id(&path.join("idVendor"))? == Some(0x0fd9)
            && read_id(&path.join("idProduct"))? == Some(0x00b6)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Inventory(std::path::PathBuf);

    impl Inventory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "wave-hotplug-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn add(&self, name: &str, vendor: &str, product: &str) {
            let path = self.0.join(name);
            fs::create_dir(&path).unwrap();
            fs::write(path.join("idVendor"), vendor).unwrap();
            fs::write(path.join("idProduct"), product).unwrap();
        }
    }

    impl Drop for Inventory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn inventory_recognizes_only_mk2_and_tracks_removal() {
        let inventory = Inventory::new();
        assert!(!attached_in(&inventory.0).unwrap());
        inventory.add("1-1", "0fd9\n", "007d\n");
        inventory.add("1-2", "1234\n", "00b6\n");
        fs::create_dir(inventory.0.join("1-1:1.0")).unwrap();
        assert!(!attached_in(&inventory.0).unwrap());
        inventory.add("1-3", "0FD9\n", "00B6\n");
        assert!(attached_in(&inventory.0).unwrap());
        fs::remove_dir_all(inventory.0.join("1-3")).unwrap();
        assert!(!attached_in(&inventory.0).unwrap());
    }

    #[test]
    fn inventory_errors_are_not_reported_as_absence() {
        let inventory = Inventory::new();
        assert!(attached_in(&inventory.0.join("missing")).is_err());
        inventory.add("1-1", "unreadable identity", "00b6\n");
        assert!(attached_in(&inventory.0).is_err());
        fs::remove_file(inventory.0.join("1-1/idVendor")).unwrap();
        fs::create_dir(inventory.0.join("1-1/idVendor")).unwrap();
        assert!(attached_in(&inventory.0).is_err());
    }
}
