//! Explicit, root-only USB rule setup; never called by the background worker.
//! A private hard-link receipt records the exact inode we installed, so even an
//! identical replacement cannot be mistaken for a rule owned by this plugin.
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const RULE: &[u8] = include_bytes!("../udev/70-sudonim-wave-xlr-mk2.rules");
const RULE_NAME: &str = "70-sudonim-wave-xlr-mk2.rules";
const STATE_DIR: &str = ".sudonim-wave-xlr-mk2";

pub fn run(action: &str) -> Result<(), String> {
    // SAFETY: geteuid has no arguments or side effects.
    if unsafe { libc::geteuid() } != 0 {
        return Err("USB rule setup requires an explicit sudo invocation".into());
    }
    let result =
        manage(Path::new("/etc/udev/rules.d"), action).map_err(|error| error.to_string())?;
    println!("{result}. Reload udev rules, then reconnect the Elgato Wave XLR MK.2.");
    Ok(())
}

fn refused(message: &str) -> io::Error {
    io::Error::other(format!(
        "Refusing USB rule change: {message}; existing files were preserved"
    ))
}

fn metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn owned(meta: &Metadata) -> bool {
    // Tests use an isolated directory owned by their effective user. Production
    // reaches this code only after run() has required effective UID zero.
    meta.uid() == unsafe { libc::geteuid() }
}

fn directory(path: &Path, private: bool) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || !owned(&meta)
        || meta.mode() & 0o022 != 0
        || (private && meta.mode() & 0o7777 != 0o700)
    {
        return Err(refused(
            "setup directory is not a trusted, non-symlink directory",
        ));
    }
    Ok(())
}

fn read_rule(path: &Path) -> io::Result<Metadata> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || !owned(&meta) || meta.mode() & 0o7777 != 0o644 {
        return Err(refused(
            "rule or ownership receipt has an unexpected type, owner or mode",
        ));
    }
    let mut contents = Vec::new();
    Read::by_ref(&mut file)
        .take(RULE.len() as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents != RULE {
        return Err(refused("rule contents have been modified"));
    }
    Ok(meta)
}

fn matching_receipt(target: &Path, receipt: &Path) -> io::Result<()> {
    let installed = read_rule(target)?;
    let recorded = read_rule(receipt)?;
    if installed.dev() != recorded.dev() || installed.ino() != recorded.ino() {
        return Err(refused(
            "this rule was not installed by this helper, or has been replaced",
        ));
    }
    Ok(())
}

fn lock(state: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state.join("lock"))?;
    let meta = file.metadata()?;
    if !meta.is_file() || !owned(&meta) || meta.mode() & 0o7777 != 0o600 || meta.nlink() != 1 {
        return Err(refused("setup lock is not a private regular file"));
    }
    // SAFETY: the owned file remains open for the complete setup operation.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(refused("another setup operation holds the lock"));
    }
    Ok(file)
}

fn manage(base: &Path, action: &str) -> io::Result<&'static str> {
    if !matches!(action, "install" | "remove") {
        return Err(refused("unknown setup action"));
    }
    directory(base, false)?;
    let target = base.join(RULE_NAME);
    let state: PathBuf = base.join(STATE_DIR);
    if metadata(&state)?.is_none() {
        if metadata(&target)?.is_some() {
            return Err(refused(
                "a rule already exists without this plugin's ownership receipt",
            ));
        }
        if action == "remove" {
            return Ok("Plugin USB rule is already absent");
        }
        DirBuilder::new().mode(0o700).create(&state)?;
    }
    directory(&state, true)?;
    let _lock = lock(&state)?;
    let receipt = state.join("installed.rules");
    if metadata(&target)?.is_some() {
        matching_receipt(&target, &receipt)?;
        if action == "install" {
            return Ok("Plugin USB rule is already installed");
        }
        fs::remove_file(&target)?;
        fs::remove_file(&receipt)?;
        return Ok("Removed this plugin's USB rule");
    }
    if action == "remove" {
        if metadata(&receipt)?.is_some() {
            read_rule(&receipt)?;
            fs::remove_file(&receipt)?;
        }
        return Ok("Plugin USB rule is already absent");
    }
    if metadata(&receipt)?.is_none() {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&receipt)?;
        file.write_all(RULE)?;
        // Set an exact, non-writable-by-others mode even under a restrictive umask.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.sync_all()?;
    }
    read_rule(&receipt)?;
    // Atomic, no-clobber publication on the same filesystem. Unlike copy/install,
    // hard_link fails if any file, directory, or dangling symlink occupies target.
    fs::hard_link(&receipt, &target)?;
    Ok("Installed this plugin's USB rule")
}

#[cfg(test)]
#[path = "udev_setup_tests.rs"]
mod tests;
