//! Optional integration tests for the actual JS/QML panel, without hardware access.

use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("wave-{name}-{}-{nonce}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn run(&self, command: &mut Command) -> String {
        // Files avoid pipe-buffer deadlocks even if Qt emits extensive diagnostics.
        let log_path = self.path("output.log");
        let log = File::create(&log_path).unwrap();
        let mut child = command
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap_or_else(|error| panic!("Cannot start {command:?}: {error}"));
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "Test process timed out or could not be reaped ({result:?}):\n{}",
                        fs::read_to_string(&log_path).unwrap()
                    );
                }
            }
        };
        let output = fs::read_to_string(log_path).unwrap();
        assert!(
            status.success(),
            "{command:?} exited with {status}:\n{output}"
        );
        output
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires Node.js; run cargo test --test panel -- --ignored"]
fn panel_state_efficiency() {
    let workspace = Workspace::new("state-test");
    let output = workspace
        .run(Command::new("node").arg(Path::new(ROOT).join("tests/fixtures/panel_efficiency.cjs")));
    assert!(
        output.contains("PASS state identity"),
        "Missing test completion:\n{output}"
    );
    print!("{output}");
}

#[test]
#[ignore = "requires Omarchy/Quickshell and QtTest; run cargo test --test panel -- --ignored"]
fn panel_slider_gestures() {
    let source = fs::read_to_string(Path::new(ROOT).join("Panel.qml")).unwrap();
    let start = source
        .find("    component Label:")
        .expect("Label component");
    let end = source[start..]
        .find("    component Effect:")
        .expect("Effect component")
        + start;
    let harness =
        include_str!("fixtures/panel_slider.qml.in").replace("@@COMPONENTS@@", &source[start..end]);
    let workspace = Workspace::new("slider-test");
    for module in ["Commons", "Ui"] {
        let source = Path::new("/usr/share/omarchy/shell").join(module);
        assert!(
            source.is_dir(),
            "Missing Omarchy module: {}",
            source.display()
        );
        symlink(source, workspace.path(module)).unwrap();
    }
    fs::write(workspace.path("shell.qml"), harness).unwrap();
    let output = workspace.run(
        Command::new("quickshell")
            .args(["--no-color", "-p"])
            .arg(workspace.path("shell.qml"))
            .env("QT_QPA_PLATFORM", "offscreen"),
    );
    let counts: Vec<_> = output
        .lines()
        .find_map(|line| {
            line.split_once("WAVE TESTS COMPLETE ")
                .map(|(_, counts)| counts)
        })
        .unwrap_or_else(|| panic!("Missing QtTest completion:\n{output}"))
        .split_whitespace()
        .take(2)
        .collect();
    // Seven gestures plus initTestCase must all run; an empty suite cannot pass.
    assert_eq!(counts, ["8", "0"], "Unexpected QtTest counts:\n{output}");
    for line in output.lines().filter(|line| line.contains("WAVE ")) {
        println!("{line}");
    }
}
