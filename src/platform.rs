use crate::{device, worker::Platform};
use serde_json::{Value, json};
use std::{
    io::Read,
    os::fd::AsRawFd,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct System;
impl Platform for System {
    fn attached(&self) -> Result<bool, String> {
        crate::hotplug::attached()
    }
    fn open(&mut self) -> Result<Box<dyn device::Device>, device::OpenError> {
        device::open()
    }
    fn present(&self) -> bool {
        std::fs::read_to_string("/proc/asound/cards").is_ok_and(|s| {
            s.lines().any(|line| {
                line.split_once('[')
                    .and_then(|(_, s)| s.split_once(']'))
                    .is_some_and(|(id, _)| id.trim() == "MK2")
            })
        })
    }
    fn alsa(&mut self, edit: Option<(&str, f64)>) -> Result<Value, String> {
        if let Some((key, value)) = edit {
            let name = control_name(key).ok_or("USB permission needed for DSP controls")?;
            let raw = match key {
                "mute" => {
                    if value == 1.0 {
                        "off".into()
                    } else {
                        "on".into()
                    }
                }
                "headphones" => (240.0 + value * 4.0).round_ties_even().to_string(),
                _ => value.trunc().to_string(),
            };
            output(
                "amixer",
                &["-q", "-c", "MK2", "cset", &format!("name={name}"), &raw],
                3000,
            )?;
        }
        parse_alsa(&output("amixer", &["-c", "MK2", "contents"], 3000)?)
    }
    fn defaults(&mut self) -> Result<(), String> {
        let nodes: Value =
            serde_json::from_str(&output("pw-dump", &[], 4000)?).map_err(|e| e.to_string())?;
        let mut sink = None;
        let mut source = None;
        for node in nodes.as_array().ok_or("Invalid PipeWire node list")? {
            let props = &node["info"]["props"];
            if !props["node.name"]
                .as_str()
                .is_some_and(|s| s.contains("Elgato_Wave_XLR_MK.2"))
            {
                continue;
            }
            let id = node["id"].as_u64();
            match props["media.class"].as_str() {
                Some("Audio/Sink") => sink = id,
                Some("Audio/Source") => source = id,
                _ => (),
            }
        }
        let (sink, source) = sink
            .zip(source)
            .ok_or("Wave input/output not both available")?;
        for id in [sink, source] {
            output("wpctl", &["set-default", &id.to_string()], 3000)?;
        }
        Ok(())
    }
}
fn control_name(key: &str) -> Option<&'static str> {
    match key {
        "gain" => Some("Elgato Wave XLR MK.2 Capture Volume"),
        "mute" => Some("Elgato Wave XLR MK.2 Capture Switch"),
        "headphones" => Some("Elgato Wave XLR MK.2 Playback Volume"),
        _ => None,
    }
}
fn parse_alsa(text: &str) -> Result<Value, String> {
    let mut values = std::collections::HashMap::new();
    let mut name = None;
    for line in text.lines() {
        if line.starts_with("numid=") {
            name = line
                .split_once("name='")
                .and_then(|(_, s)| s.split_once('\''))
                .map(|(s, _)| s);
        }
        if let Some(raw) = line.trim().strip_prefix(": values=")
            && let Some(name) = name
        {
            values.insert(name, raw.split(',').next().unwrap_or(""));
        }
    }
    let get = |key| {
        values
            .get(control_name(key).unwrap())
            .copied()
            .ok_or("ALSA control missing".to_string())
    };
    let gain: i64 = get("gain")?.parse().map_err(|_| "Invalid ALSA gain")?;
    let hp: i64 = get("headphones")?
        .parse()
        .map_err(|_| "Invalid ALSA headphones")?;
    let mute = match get("mute")? {
        "off" => true,
        "on" => false,
        _ => return Err("Invalid ALSA mute".into()),
    };
    if !(0..=80).contains(&gain) || !(0..=240).contains(&hp) {
        return Err("Unexpected ALSA control range".into());
    }
    Ok(
        json!({"connected":true,"usb":false,"gain":gain,"mute":mute,"headphones":(hp as f64-240.0)/4.0,
        "note":"Basic controls available · DSP needs USB access"}),
    )
}

// Drain bounded subprocess output while waiting. No helper thread, shell expansion,
// or child that survives a timeout. Used only for ALSA fallback / explicit defaults.
fn output(program: &str, args: &[&str], timeout_ms: u64) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let result = (|| {
        let mut pipe = child.stdout.take().ok_or("Missing child stdout")?;
        let fd = pipe.as_raw_fd();
        // SAFETY: fd remains owned by pipe for the entire loop.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut bytes = Vec::new();
        let mut buf = [0u8; 8192];
        let mut eof = false;
        loop {
            if Instant::now() >= deadline {
                return Err(format!("{program} timed out"));
            }
            if !eof {
                match pipe.read(&mut buf) {
                    Ok(0) => eof = true,
                    Ok(n) => {
                        if bytes.len() + n > 4 * 1024 * 1024 {
                            return Err(format!("{program} output exceeds 4 MiB"));
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.to_string()),
                }
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("{program} failed ({status})"));
                }
                if eof {
                    return String::from_utf8(bytes).map_err(|e| e.to_string());
                }
            }
            let ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 20) as i32;
            // SAFETY: valid stack pollfd; fd -1 is deliberately ignored after EOF.
            let mut pfd = libc::pollfd {
                fd: if eof { -1 } else { fd },
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut pfd, 1, ms) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_all_alsa_fields() {
        let text = "numid=1,iface=MIXER,name='Elgato Wave XLR MK.2 Capture Volume'\n : values=43\nnumid=2,name='Elgato Wave XLR MK.2 Playback Volume'\n : values=160,160\nnumid=3,name='Elgato Wave XLR MK.2 Capture Switch'\n : values=off\n";
        let state = parse_alsa(text).unwrap();
        assert_eq!(state["gain"], 43);
        assert_eq!(state["headphones"], -20.0);
        assert_eq!(state["mute"], true);
        assert!(parse_alsa(&text.replace("values=43", "values=99")).is_err());
        assert!(parse_alsa("").is_err());
    }
    #[test]
    fn child_failure_and_timeout() {
        assert!(output("false", &[], 500).is_err());
        let start = Instant::now();
        assert!(
            output("sleep", &["2"], 30)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn drain_more_than_pipe_capacity() {
        let text = output("head", &["-c", "131072", "/dev/zero"], 2000).unwrap();
        assert_eq!(text.len(), 131072);
    }
}
