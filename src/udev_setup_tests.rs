use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::{SystemTime, UNIX_EPOCH};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("wave-udev-test-{}-{nonce}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn target(&self) -> PathBuf {
        self.0.join(RULE_NAME)
    }
    fn receipt(&self) -> PathBuf {
        self.0.join(STATE_DIR).join("installed.rules")
    }
    fn run(&self, action: &str) -> io::Result<&'static str> {
        manage(&self.0, action)
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn installs_idempotently_and_removes_only_recorded_inode() {
    let dir = Sandbox::new();
    let legacy = dir.0.join("70-wave-xlr-mk2.rules");
    fs::write(&legacy, b"existing user rule").unwrap();
    dir.run("install").unwrap();
    assert_eq!(fs::read(dir.target()).unwrap(), RULE);
    assert_eq!(
        fs::metadata(dir.target()).unwrap().ino(),
        fs::metadata(dir.receipt()).unwrap().ino()
    );
    assert_eq!(fs::metadata(dir.target()).unwrap().mode() & 0o777, 0o644);
    assert_eq!(
        dir.run("install").unwrap(),
        "Plugin USB rule is already installed"
    );
    dir.run("remove").unwrap();
    assert!(!dir.target().exists());
    assert!(!dir.receipt().exists());
    dir.run("remove").unwrap();
    assert_eq!(fs::read(legacy).unwrap(), b"existing user rule");
}

#[test]
fn unowned_even_identical_existing_rule_is_never_adopted() {
    for contents in [b"foreign rule".as_slice(), RULE] {
        let dir = Sandbox::new();
        fs::write(dir.target(), contents).unwrap();
        for action in ["install", "remove"] {
            assert!(dir.run(action).is_err());
            assert_eq!(fs::read(dir.target()).unwrap(), contents);
        }
        assert!(!dir.receipt().exists());
    }
}

#[test]
fn preserves_modified_rule_and_identical_replacement() {
    for replace in [false, true] {
        let dir = Sandbox::new();
        dir.run("install").unwrap();
        if replace {
            fs::remove_file(dir.target()).unwrap();
            fs::write(dir.target(), RULE).unwrap();
        } else {
            fs::write(dir.target(), b"user modification").unwrap();
        }
        let before = fs::read(dir.target()).unwrap();
        for action in ["install", "remove"] {
            assert!(dir.run(action).is_err());
            assert_eq!(fs::read(dir.target()).unwrap(), before);
        }
    }
}

#[test]
fn symlinks_and_directories_are_preserved_without_following() {
    let dir = Sandbox::new();
    let outside = dir.0.join("outside");
    fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, dir.target()).unwrap();
    assert!(dir.run("install").is_err());
    assert!(dir.run("remove").is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    fs::remove_file(&outside).unwrap(); // dangling symlink must still count as occupied
    assert!(dir.run("install").is_err());
    fs::remove_file(dir.target()).unwrap();
    fs::create_dir(dir.target()).unwrap();
    assert!(dir.run("install").is_err());
    assert!(dir.run("remove").is_err());
    assert!(dir.target().is_dir());
}

#[test]
fn rejects_untrusted_state_directory_and_receipt() {
    let dir = Sandbox::new();
    let state = dir.0.join(STATE_DIR);
    symlink(&dir.0, &state).unwrap();
    assert!(dir.run("install").is_err());
    fs::remove_file(&state).unwrap();
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(dir.run("install").is_err());
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(dir.0.join("missing"), dir.receipt()).unwrap();
    assert!(dir.run("install").is_err());
    assert!(!dir.target().exists());
}

#[test]
fn refuses_lost_receipt_and_changed_modes() {
    let dir = Sandbox::new();
    dir.run("install").unwrap();
    fs::set_permissions(dir.target(), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(dir.run("remove").is_err());
    fs::set_permissions(dir.target(), fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(dir.receipt()).unwrap();
    assert!(dir.run("remove").is_err());
    assert!(dir.run("install").is_err());
    assert_eq!(fs::read(dir.target()).unwrap(), RULE);
}

#[test]
fn recovers_interrupted_publication_without_overwriting_a_collision() {
    let dir = Sandbox::new();
    dir.run("install").unwrap();
    fs::remove_file(dir.target()).unwrap();
    // Receipt was written but publication did not complete: reuse its inode.
    dir.run("install").unwrap();
    matching_receipt(&dir.target(), &dir.receipt()).unwrap();
    fs::remove_file(dir.target()).unwrap();
    fs::write(dir.target(), b"intervening install").unwrap();
    assert!(dir.run("install").is_err());
    assert_eq!(fs::read(dir.target()).unwrap(), b"intervening install");
}

#[test]
fn serializes_setup_without_probing_hardware() {
    let dir = Sandbox::new();
    dir.run("install").unwrap();
    let _held = lock(&dir.0.join(STATE_DIR)).unwrap();
    assert!(dir.run("remove").is_err());
    assert_eq!(fs::read(dir.target()).unwrap(), RULE);
}

#[test]
fn bootstrap_rejects_tampering_and_pins_running_image_across_path_replacement() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = Sandbox::new();
    let executable = dir.0.join("worker");
    fs::copy("/usr/bin/sleep", &executable).unwrap();
    let child = ChildGuard(Command::new(&executable).arg("30").spawn().unwrap());
    let pinned = format!("/proc/{}/exe", child.0.id());
    let digest = executable_digest(&pinned).unwrap();
    // Model replacement of the user-writable checkout while authentication waits.
    fs::rename(&executable, dir.0.join("original")).unwrap();
    fs::copy("/usr/bin/false", &executable).unwrap();
    assert_eq!(executable_digest(&pinned).unwrap(), digest);
    assert_ne!(
        executable_digest(executable.to_str().unwrap()).unwrap(),
        digest
    );

    let invoke = |hash: &str| {
        Command::new("/usr/bin/bash")
            .args([
                "--noprofile",
                "--norc",
                "-p",
                "-c",
                BOOTSTRAP,
                "test",
                &pinned,
                hash,
                "--install-udev-rule",
            ])
            .output()
            .unwrap()
    };
    let tampered = invoke(&"0".repeat(64));
    assert!(!tampered.status.success());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("refusing privileged execution"));
    let original = invoke(&digest);
    // sleep rejects the setup flag: reaching its error proves that the private
    // copy of the original image ran, rather than the replacement `false` binary.
    let stderr = String::from_utf8_lossy(&original.stderr);
    assert!(
        stderr.contains("unrecognized option '--install-udev-rule'"),
        "{stderr}"
    );
    let private_executable = stderr.lines().next().unwrap().split(':').next().unwrap();
    assert!(private_executable.starts_with("/tmp/omarchy-wave-xlr-setup."));
    assert!(
        !Path::new(private_executable).exists(),
        "private copy must be cleaned up"
    );
}
