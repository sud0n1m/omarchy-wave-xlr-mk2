mod device;
mod framing;
mod hotplug;
mod platform;
mod worker;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::fd::AsRawFd,
    time::Instant,
};
use worker::Worker;

fn emit(message: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, message)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}
fn run() -> Result<(), String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }))
        });
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(runtime.join("wave-xlr-control.lock"))
        .map_err(|e| e.to_string())?;
    // SAFETY: the owned file stays open until all device handles have been dropped.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(
            "Wave XLR owner already running; send commands through its NDJSON stdin".into(),
        );
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Listen before inventory so an attachment during startup cannot be missed.
    let mut hotplug = hotplug::Hotplug::new()?;
    let start = Instant::now();
    let mut owner = Worker::new(platform::System, move || start.elapsed().as_millis() as u64);
    owner.sample(true);
    if args.first().map(String::as_str) != Some("--watch") {
        if let Some(key) = args.first() {
            let command = if key == "defaults" {
                json!({"id":1,"op":"defaults"})
            } else {
                let value: f64 = args
                    .get(1)
                    .ok_or("Missing control value")?
                    .parse()
                    .map_err(|_| "Invalid control value")?;
                json!({"id":1,"op":"set","key":key,"value":value})
            };
            owner.command(command);
            for event in owner.take_events() {
                if event["type"] == "result" && event["ok"] != true {
                    return Err(event["error"].as_str().unwrap_or("Command failed").into());
                }
            }
        }
        if owner.state["connected"] != true {
            return Err(owner.state["error"]
                .as_str()
                .unwrap_or("Device disconnected or inaccessible")
                .into());
        }
        emit(&owner.state).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if args.len() != 1 {
        return Err("Usage: wave-xlr-control [--watch | <control> <value> | defaults]".into());
    }
    let mut buf = [0u8; 4096];
    let mut framer = framing::Framer::default();
    loop {
        owner.tick();
        for event in owner.take_events() {
            if let Err(e) = emit(&event) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.to_string());
            }
        }
        let mut pfds = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: hotplug.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two initialized pollfds, both descriptors alive for the loop.
        let ready = unsafe {
            libc::poll(
                pfds.as_mut_ptr(),
                pfds.len() as libc::nfds_t,
                owner
                    .wait_ms()
                    .map_or(-1, |ms| ms.min(i32::MAX as u64) as i32),
            )
        };
        if ready < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.to_string());
        }
        if ready == 0 {
            continue;
        }
        if pfds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err("USB event monitor stopped".into());
        }
        if pfds[1].revents & libc::POLLIN != 0 && hotplug.changed() {
            owner.hardware_changed();
            // Deliver the wake notice before tick()/commands can touch USB.
            for event in owner.take_events() {
                emit(&event).map_err(|e| e.to_string())?;
            }
        }
        if pfds[0].revents == 0 {
            continue;
        }
        // Use raw read, not a buffered reader that could hide ready frames from poll.
        // SAFETY: buf is a writable array of the passed length; fd 0 is stdin.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 {
            return Ok(());
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.to_string());
        }
        let n = n as usize;
        for byte in &buf[..n] {
            if let Some(result) = framer.feed(*byte) {
                match result {
                    Ok(message) => owner.command(message),
                    Err(error) => {
                        emit(&json!({"type":"result","id":null,"ok":false,"error":error}))
                            .map_err(|e| e.to_string())?
                    }
                }
                owner.tick();
                for event in owner.take_events() {
                    emit(&event).map_err(|e| e.to_string())?;
                }
            }
        }
    }
}
fn main() {
    if let Err(error) = run() {
        let _ = emit(&json!({"connected":false,"error":error}));
        std::process::exit(1);
    }
}
