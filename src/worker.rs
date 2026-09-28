use crate::device::{self, Device, OpenError};
use serde_json::{Value, json};

pub trait Platform {
    fn attached(&self) -> Result<bool, String>;
    fn open(&mut self) -> Result<Box<dyn Device>, OpenError>;
    fn present(&self) -> bool;
    fn alsa(&mut self, edit: Option<(&str, f64)>) -> Result<Value, String>;
    fn defaults(&mut self) -> Result<(), String>;
}

pub struct Worker<P, C> {
    pub state: Value,
    pub next_poll: u64,
    pub interval: u64,
    next_connect: u64,
    backoff: u64,
    last_emit: Option<u64>,
    sampled_at: u64,
    seq: u64,
    device: Option<Box<dyn Device>>,
    fallback: bool,
    attached: Option<bool>,
    platform: P,
    clock: C,
    events: Vec<Value>,
}

impl<P: Platform, C: Fn() -> u64> Worker<P, C> {
    pub fn new(platform: P, clock: C) -> Self {
        Self {
            state: json!({"connected":false,"usb":false,"stale":true}),
            next_poll: 0,
            interval: 500,
            next_connect: 0,
            backoff: 250,
            last_emit: None,
            sampled_at: 0,
            seq: 0,
            device: None,
            fallback: false,
            attached: None,
            platform,
            clock,
            events: Vec::new(),
        }
    }
    fn now(&self) -> u64 {
        (self.clock)()
    }
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
    fn publish(&mut self, state: Value, force: bool, sampled: bool) {
        let now = self.now();
        if sampled {
            self.sampled_at = now;
        }
        let changed = state != self.state;
        self.state = state;
        if changed
            || force
            || (self.attached != Some(false)
                && self.last_emit.is_none_or(|t| now.saturating_sub(t) >= 1000))
        {
            self.seq += 1;
            self.events.push(json!({"type":"state","state":self.state,"seq":self.seq,"sampledAtMs":self.sampled_at}));
            self.last_emit = Some(now);
        }
    }
    fn failed(&mut self, error: String) {
        self.device = None;
        self.next_connect = self.now() + self.backoff;
        self.backoff = (self.backoff * 2).min(5000);
        let mut state = self.state.clone();
        state["connected"] = false.into();
        state["usb"] = false.into();
        state["stale"] = true.into();
        match self.attached {
            Some(present) => state["present"] = present.into(),
            None => {
                state.as_object_mut().unwrap().remove("present");
            }
        }
        state["error"] = error.into();
        self.publish(state, false, false);
    }
    pub fn hardware_changed(&mut self) {
        // Inventory is passive. Queue a wake notice BEFORE potentially blocking USB
        // work so the shell can resume its watchdog before our next tick opens USB.
        let was_absent = self.attached == Some(false);
        self.attached = None;
        self.next_connect = 0;
        self.backoff = 250;
        match self.platform.attached() {
            Ok(attached) => {
                self.attached = Some(attached);
                if !attached {
                    self.sample(true);
                    return;
                }
                if was_absent {
                    self.publish(
                        json!({"connected":false,"usb":false,"present":true,"stale":true}),
                        true,
                        false,
                    );
                }
                self.next_poll = self.now();
            }
            Err(error) => {
                self.failed(error);
                self.next_poll = self.now() + 5000;
            }
        }
    }
    pub fn sample(&mut self, force: bool) {
        if self.attached.is_none() {
            match self.platform.attached() {
                Ok(attached) => self.attached = Some(attached),
                Err(error) => {
                    self.failed(error);
                    self.next_poll = self.now() + 5000;
                    return;
                }
            }
        }
        if self.attached == Some(false) {
            self.device = None;
            self.fallback = false;
            self.next_poll = u64::MAX;
            self.publish(
                json!({"connected":false,"usb":false,"stale":false,"present":false}),
                force,
                false,
            );
            return;
        }
        if self.device.is_none() && self.now() >= self.next_connect {
            match self.platform.open() {
                Ok(device) => {
                    self.device = Some(device);
                    self.fallback = false;
                }
                Err(error) => {
                    self.fallback = error.permission;
                    if self.fallback
                        && self.platform.present()
                        && self.state["connected"] == true
                        && self.state["usb"] == false
                    {
                        self.next_connect = self.now() + self.backoff;
                        self.backoff = (self.backoff * 2).min(5000);
                    } else {
                        self.failed(error.message);
                    }
                }
            }
        }
        let read = if let Some(device) = self.device.as_mut() {
            Some(device.read().and_then(|b| device::decode(&b)))
        } else if self.fallback && self.platform.present() {
            Some(self.platform.alsa(None))
        } else {
            if self.state["connected"] == true {
                self.failed("Wave XLR MK.2 disconnected".into());
            }
            None
        };
        match read {
            Some(Ok(mut state)) => {
                if self.device.is_some() {
                    self.backoff = 250;
                }
                state["stale"] = false.into();
                state["present"] = true.into();
                self.publish(state, force, true);
            }
            Some(Err(error)) => self.failed(error),
            None => self.publish(self.state.clone(), force, false),
        }
        self.next_poll = if self.device.is_some() {
            self.now() + self.interval
        } else if self.state["connected"] == true && self.state["stale"] == false {
            self.now() + if self.interval == 50 { 500 } else { 2000 }
        } else {
            (self.now() + 50).max(self.next_connect)
        };
    }
    pub fn tick(&mut self) {
        if self.attached == Some(false) {
            return;
        }
        if self.now() >= self.next_poll {
            self.sample(false);
        } else if self
            .last_emit
            .is_none_or(|t| self.now().saturating_sub(t) >= 1000)
        {
            self.publish(self.state.clone(), false, false);
        }
    }
    pub fn wait_ms(&self) -> Option<u64> {
        if self.attached == Some(false) {
            return None;
        }
        Some(
            self.next_poll
                .min(self.last_emit.map_or(0, |t| t + 1000))
                .saturating_sub(self.now()),
        )
    }
    fn execute(&mut self, message: &Value) -> Result<(), String> {
        let id = &message["id"];
        if !message.is_object() || !(id.is_string() || id.is_i64() || id.is_u64()) {
            return Err("Command id must be a string or integer".into());
        }
        match message["op"].as_str() {
            Some("poll") => {
                let interval = message["intervalMs"]
                    .as_u64()
                    .filter(|v| *v == 50 || *v == 500)
                    .ok_or("intervalMs must be 50 (open) or 500 (closed)")?;
                self.interval = interval;
                self.sample(true);
            }
            Some("refresh") => self.sample(true),
            Some("defaults" | "set") => {
                if self.state["connected"] != true || self.state["stale"] != false {
                    return Err(
                        "Device is disconnected; refresh after reconnection before editing".into(),
                    );
                }
                if message["op"] == "defaults" {
                    return self.platform.defaults();
                }
                let key = message["key"]
                    .as_str()
                    .ok_or("Control key must be a string")?;
                let value = message["value"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or("Control value must be a finite number")?;
                // Validate before touching hardware. Invalid commands never disconnect it.
                device::modify(
                    &device::Blocks {
                        blend: [0; 6],
                        settings: [0; 38],
                        headphones: [0; 2],
                    },
                    key,
                    value,
                )?;
                if self.device.is_none() && !["gain", "headphones", "mute"].contains(&key) {
                    return Err("USB permission needed for DSP controls".into());
                }
                let outcome = if let Some(device) = self.device.as_mut() {
                    (|| {
                        let mut before = device.read()?;
                        device::decode(&before)?;
                        let (block, bytes) = device::modify(&before, key, value)?;
                        device.write(block, &bytes)?;
                        let state = device::decode(&device.read()?)?;
                        match block {
                            1 => before.blend.copy_from_slice(&bytes),
                            4 => before.settings.copy_from_slice(&bytes),
                            5 => before.headphones.copy_from_slice(&bytes),
                            _ => return Err("Unknown block".into()),
                        }
                        let confirmed = state[key] == device::decode(&before)?[key];
                        Ok((state, confirmed))
                    })()
                } else {
                    self.platform.alsa(Some((key, value))).map(|state| {
                        let expected = match key {
                            "mute" => Value::Bool(value == 1.0),
                            "gain" => json!(value.trunc() as i64),
                            _ => json!((value * 4.0).round_ties_even() / 4.0),
                        };
                        let confirmed = state[key] == expected;
                        (state, confirmed)
                    })
                };
                match outcome {
                    Ok((mut state, confirmed)) => {
                        state["stale"] = false.into();
                        state["present"] = true.into();
                        self.publish(state, true, true);
                        if !confirmed {
                            return Err("Device did not confirm the requested setting".into());
                        }
                    }
                    Err(error) => {
                        self.failed(error.clone());
                        return Err(error);
                    }
                }
            }
            _ => return Err("Unknown operation".into()),
        }
        Ok(())
    }
    pub fn command(&mut self, message: Value) {
        let result = self.execute(&message);
        let mut event =
            json!({"type":"result","id":message.get("id"),"ok":result.is_ok(),"state":self.state});
        if let Err(error) = result {
            event["error"] = error.into();
        }
        self.events.push(event);
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
