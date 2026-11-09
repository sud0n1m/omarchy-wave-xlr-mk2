use crate::control_events::{BLEND, HEADPHONES, SETTINGS};
use crate::device::{self, Device, OpenError};
use serde_json::{Value, json};

pub trait Platform {
    fn attached(&self) -> Result<bool, String>;
    fn open(&mut self) -> Result<Box<dyn Device>, OpenError>;
    fn present(&self) -> bool;
    fn alsa(&mut self, edit: Option<(&str, f64)>) -> Result<Value, String>;
    fn defaults(&mut self) -> Result<(), String>;
}

enum Frame {
    Full,
    Patch(serde_json::Map<String, Value>),
    Liveness,
}

fn steady(state: &Value) -> bool {
    state["connected"] == true && state["stale"] == false
}

/// The changed fields between two steady states with identical keys; None when
/// a whole frame is needed.
fn patch(old: &Value, new: &Value) -> Option<serde_json::Map<String, Value>> {
    let (Value::Object(old_map), Value::Object(new_map)) = (old, new) else {
        return None;
    };
    if !steady(old) || !steady(new) || old_map.len() != new_map.len() {
        return None;
    }
    let mut changed = serde_json::Map::new();
    for (key, value) in new_map {
        match old_map.get(key) {
            Some(previous) if previous == value => {}
            Some(_) => {
                changed.insert(key.clone(), value.clone());
            }
            None => return None,
        }
    }
    Some(changed)
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
    /// Raw blocks behind the current state; an identical read skips decoding.
    decoded: Option<device::Blocks>,
    fallback: bool,
    attached: Option<bool>,
    /// Device-side changes arrive as ALSA control events, so the closed panel
    /// needs only a slow safety poll.
    pushed: bool,
    /// Blocks the device reported changed since the last sample (control_events bits).
    dirty: u8,
    /// When the next full three-block read is due; event samples read less.
    full_due: u64,
    /// With events and an open panel: when the dial blocks are next read.
    dial_due: u64,
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
            decoded: None,
            fallback: false,
            attached: None,
            pushed: false,
            dirty: 0,
            full_due: 0,
            dial_due: 0,
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
    /// Emit a state frame. Full state goes out when it changed or a command
    /// asked for it; an unchanged heartbeat is a small liveness-only frame.
    /// Emit a state frame: the full state, only the changed fields, or (for
    /// an unchanged heartbeat) no state at all.
    fn emit(&mut self, frame: Frame, now: u64) {
        self.seq += 1;
        let mut event = json!({"type":"state","seq":self.seq,"sampledAtMs":self.sampled_at});
        match frame {
            Frame::Full => event["state"] = self.state.clone(),
            Frame::Patch(patch) => event["patch"] = Value::Object(patch),
            Frame::Liveness => {}
        }
        self.events.push(event);
        self.last_emit = Some(now);
    }
    fn heartbeat_due(&self, now: u64) -> bool {
        self.attached != Some(false) && self.last_emit.is_none_or(|t| now.saturating_sub(t) >= 1000)
    }
    fn publish(&mut self, state: Value, force: bool, sampled: bool) {
        let now = self.now();
        if sampled {
            self.sampled_at = now;
        }
        self.decoded = None;
        // Between two steady connected states with the same fields, send only
        // what changed; anything else (and every forced frame) is sent whole.
        let frame = match patch(&self.state, &state) {
            Some(patch) if patch.is_empty() => None,
            Some(patch) if !force => Some(Frame::Patch(patch)),
            Some(_) => Some(Frame::Full),
            None => (state != self.state).then_some(Frame::Full),
        };
        self.state = state;
        match frame {
            Some(frame) => self.emit(frame, now),
            None if force => self.emit(Frame::Full, now),
            None if self.heartbeat_due(now) => self.emit(Frame::Liveness, now),
            None => {}
        }
    }
    /// The state is unchanged: no clone, comparison or full serialization.
    fn keep(&mut self, force: bool, sampled: bool) {
        let now = self.now();
        if sampled {
            self.sampled_at = now;
        }
        if force {
            self.emit(Frame::Full, now);
        } else if self.heartbeat_due(now) {
            self.emit(Frame::Liveness, now);
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
    /// Slow safety poll while closed when the device pushes its own changes.
    pub const EVENT_SAFETY_POLL_MS: u64 = 5000;
    /// How often every block is read.
    fn poll_interval(&self) -> u64 {
        if self.pushed {
            Self::EVENT_SAFETY_POLL_MS
        } else {
            self.interval
        }
    }
    /// With events, an open panel still polls the blocks the dial changes
    /// without an event (monitor blend, Voice Tune strength) at its interval.
    fn dial_polling(&self) -> bool {
        self.pushed && self.interval == 50
    }
    fn device_due(&self) -> u64 {
        if self.dial_polling() {
            self.full_due.min(self.dial_due)
        } else {
            self.full_due
        }
    }
    pub fn attached(&self) -> Option<bool> {
        self.attached
    }
    /// Whether a control-event subscription is active.
    pub fn set_events(&mut self, active: bool) {
        self.pushed = active;
        if self.device.is_some() {
            let now = self.now();
            self.full_due = self.full_due.min(now + self.poll_interval());
            self.dial_due = self.dial_due.min(now + self.interval);
            self.next_poll = self.next_poll.min(self.device_due());
        }
    }
    /// The device reported a change in these blocks: read just those on the
    /// next tick, without waiting for the poll interval.
    pub fn device_signaled(&mut self, blocks: u8) {
        if self.attached != Some(false) && self.device.is_some() {
            self.dirty |= blocks;
            self.next_poll = self.next_poll.min(self.now());
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
        // Forced samples, the full-poll schedule and a missing cache read all
        // three blocks. Otherwise only the blocks an event reported, plus the
        // dial blocks when their poll is due, are refreshed on the cache.
        let now = self.now();
        let dirty = std::mem::take(&mut self.dirty);
        let full = force || self.decoded.is_none() || now >= self.full_due;
        let dial = !full && self.dial_polling() && now >= self.dial_due;
        let wanted = dirty | if dial { BLEND | SETTINGS } else { 0 };
        let partial = !full && wanted != 0;
        let read = if let Some(device) = self.device.as_mut() {
            let fresh = match self.decoded.as_ref() {
                Some(cached) if partial => {
                    let mut blocks = cached.clone();
                    [(BLEND, 1), (SETTINGS, 4), (HEADPHONES, 5)]
                        .into_iter()
                        .filter(|(bit, _)| wanted & bit != 0)
                        .try_for_each(|(_, block)| device.read_one(block, &mut blocks))
                        .map(|()| blocks)
                }
                _ => device.read(),
            };
            Some(match fresh {
                Ok(blocks) if self.decoded.as_ref() == Some(&blocks) => Ok(None),
                Ok(blocks) => device::decode(&blocks).map(|state| Some((state, Some(blocks)))),
                Err(error) => Err(error),
            })
        } else if self.fallback && self.platform.present() {
            Some(self.platform.alsa(None).map(|state| Some((state, None))))
        } else {
            if self.state["connected"] == true {
                self.failed("Wave XLR MK.2 disconnected".into());
            }
            None
        };
        match read {
            Some(Ok(Some((mut state, blocks)))) => {
                if self.device.is_some() {
                    self.backoff = 250;
                }
                state["stale"] = false.into();
                state["present"] = true.into();
                self.publish(state, force, true);
                self.decoded = blocks;
            }
            Some(Ok(None)) => self.keep(force, true),
            Some(Err(error)) => self.failed(error),
            None => self.keep(force, false),
        }
        self.next_poll = if self.device.is_some() {
            if !partial {
                self.full_due = now + self.poll_interval();
            }
            if !partial || dial {
                self.dial_due = now + self.interval;
            }
            self.device_due()
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
        } else {
            self.keep(false, false);
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
                device::check_command(key, value)?;
                if self.device.is_none() && !["gain", "headphones", "mute"].contains(&key) {
                    return Err("USB permission needed for DSP controls".into());
                }
                // Refuse EQ edits the last sample does not offer, before any
                // transfer, so a refusal never looks like a transport failure.
                if device::is_eq_control(key) && self.state.get(key).is_none() {
                    return Err(device::eq_unavailable(
                        self.state["eqUnavailable"].as_str().unwrap_or(""),
                    ));
                }
                let target = device::block_of(key, value)?;
                let cached = self.decoded.clone();
                let outcome = if let Some(device) = self.device.as_mut() {
                    // Read-modify-write only the target block, freshly read;
                    // the other blocks come from the last sample when cached.
                    (|| {
                        let mut before = match cached {
                            Some(mut blocks) => {
                                device.read_one(target, &mut blocks)?;
                                blocks
                            }
                            None => device.read()?,
                        };
                        // modify() validates the fresh blocks before any write.
                        let (block, bytes) = device::modify(&before, key, value)?;
                        device.write(block, &bytes)?;
                        let mut after = before.clone();
                        device.read_one(block, &mut after)?;
                        let state = device::decode(&after)?;
                        device::set_block(&mut before, block, &bytes)?;
                        let confirmed = state[key] == device::decode(&before)?[key];
                        Ok((state, confirmed, Some(after)))
                    })()
                } else {
                    self.platform.alsa(Some((key, value))).map(|state| {
                        let expected = match key {
                            "mute" => Value::Bool(value == 1.0),
                            "gain" => json!(value.trunc() as i64),
                            _ => json!((value * 4.0).round_ties_even() / 4.0),
                        };
                        let confirmed = state[key] == expected;
                        (state, confirmed, None)
                    })
                };
                match outcome {
                    Ok((mut state, confirmed, blocks)) => {
                        state["stale"] = false.into();
                        state["present"] = true.into();
                        self.publish(state, false, true);
                        self.decoded = blocks;
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
    /// The result carries no state: every state change was already published
    /// as a frame before it, in order.
    pub fn command(&mut self, message: Value) {
        let result = self.execute(&message);
        let mut event = json!({"type":"result","id":message.get("id"),"ok":result.is_ok()});
        if let Err(error) = result {
            event["error"] = error.into();
        }
        self.events.push(event);
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
