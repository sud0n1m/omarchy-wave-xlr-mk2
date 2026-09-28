use super::*;
use crate::device::Blocks;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct FakeState {
    blocks: Blocks,
    opens: usize,
    reads: usize,
    drops: usize,
    writes: Vec<(u16, Vec<u8>)>,
    open_error: Option<(String, bool)>,
    read_error: bool,
    write_error: bool,
    ignore_write: bool,
    concurrent_strength: Option<u8>,
    attached: bool,
    attached_checks: usize,
    present: bool,
    present_checks: usize,
    alsa_calls: Vec<Option<(String, f64)>>,
    alsa_state: Value,
    defaults_calls: usize,
    defaults_error: bool,
}

impl FakeState {
    fn new() -> Self {
        let mut blocks = Blocks {
            blend: [0; 6],
            settings: [0; 38],
            headphones: [0; 2],
        };
        blocks.settings[0] = 43;
        Self {
            blocks,
            opens: 0,
            reads: 0,
            drops: 0,
            writes: Vec::new(),
            open_error: None,
            read_error: false,
            write_error: false,
            ignore_write: false,
            concurrent_strength: None,
            attached: true,
            attached_checks: 0,
            present: true,
            present_checks: 0,
            alsa_calls: Vec::new(),
            alsa_state: json!({"connected":true,"usb":false,"gain":43,"headphones":-20.0,"mute":false}),
            defaults_calls: 0,
            defaults_error: false,
        }
    }
}

struct FakeDevice(Rc<RefCell<FakeState>>);

impl Device for FakeDevice {
    fn read(&mut self) -> Result<Blocks, String> {
        let mut shared = self.0.borrow_mut();
        shared.reads += 1;
        if shared.read_error {
            Err("Device disconnected during read".into())
        } else {
            Ok(shared.blocks.clone())
        }
    }

    fn write(&mut self, block: u16, bytes: &[u8]) -> Result<(), String> {
        let mut shared = self.0.borrow_mut();
        shared.writes.push((block, bytes.to_vec()));
        if shared.write_error {
            return Err("Device disconnected during write".into());
        }
        if !shared.ignore_write {
            match block {
                1 => shared.blocks.blend.copy_from_slice(bytes),
                4 => shared.blocks.settings.copy_from_slice(bytes),
                5 => shared.blocks.headphones.copy_from_slice(bytes),
                _ => return Err("Unexpected block".into()),
            }
        }
        if let Some(value) = shared.concurrent_strength {
            shared.blocks.settings[10] = value;
        }
        Ok(())
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.0.borrow_mut().drops += 1;
    }
}

struct FakePlatform(Rc<RefCell<FakeState>>);

impl Platform for FakePlatform {
    fn attached(&self) -> Result<bool, String> {
        let mut shared = self.0.borrow_mut();
        shared.attached_checks += 1;
        Ok(shared.attached)
    }

    fn open(&mut self) -> Result<Box<dyn Device>, OpenError> {
        let mut shared = self.0.borrow_mut();
        shared.opens += 1;
        if let Some((message, permission)) = &shared.open_error {
            Err(OpenError {
                message: message.clone(),
                permission: *permission,
            })
        } else {
            Ok(Box::new(FakeDevice(self.0.clone())))
        }
    }

    fn present(&self) -> bool {
        let mut shared = self.0.borrow_mut();
        shared.present_checks += 1;
        shared.present
    }

    fn alsa(&mut self, edit: Option<(&str, f64)>) -> Result<Value, String> {
        let mut shared = self.0.borrow_mut();
        shared
            .alsa_calls
            .push(edit.map(|(key, value)| (key.to_owned(), value)));
        if let Some((key, value)) = edit.filter(|_| !shared.ignore_write) {
            shared.alsa_state[key] = match key {
                "mute" => Value::Bool(value == 1.0),
                "gain" => json!(value.trunc() as i64),
                _ => json!((value * 4.0).round_ties_even() / 4.0),
            };
        }
        Ok(shared.alsa_state.clone())
    }

    fn defaults(&mut self) -> Result<(), String> {
        let mut shared = self.0.borrow_mut();
        shared.defaults_calls += 1;
        if shared.defaults_error {
            Err("Wave input/output not both available".into())
        } else {
            Ok(())
        }
    }
}

#[allow(clippy::type_complexity)] // Fixture returns owner, controllable clock and shared fake device.
fn fixture() -> (
    Worker<FakePlatform, impl Fn() -> u64>,
    Rc<Cell<u64>>,
    Rc<RefCell<FakeState>>,
) {
    let now = Rc::new(Cell::new(1000));
    let clock = now.clone();
    let shared = Rc::new(RefCell::new(FakeState::new()));
    (
        Worker::new(FakePlatform(shared.clone()), move || clock.get()),
        now,
        shared,
    )
}

fn command<C: Fn() -> u64>(worker: &mut Worker<FakePlatform, C>, message: Value) -> Value {
    worker.command(message);
    let event = worker
        .take_events()
        .pop()
        .expect("command must return a result");
    assert_eq!(event["type"], "result");
    event
}

#[test]
fn absent_startup_sleeps_without_heartbeats_or_device_probes() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().attached = false;
    worker.sample(false);
    assert_eq!(
        worker.state,
        json!({"connected":false,"usb":false,"stale":false,"present":false})
    );
    assert_eq!(worker.next_poll, u64::MAX);
    assert_eq!(worker.wait_ms(), None);
    worker.take_events();

    for instant in [2000, 3000, 4000, 10000, 60000] {
        now.set(instant);
        worker.tick();
        let events = worker.take_events();
        assert!(
            events.is_empty(),
            "known absence must not generate heartbeats"
        );
        assert_eq!(worker.wait_ms(), None);
        for message in [
            json!({"id":1,"op":"refresh"}),
            json!({"id":2,"op":"poll","intervalMs":50}),
            json!({"id":3,"op":"poll","intervalMs":500}),
        ] {
            assert_eq!(command(&mut worker, message)["ok"], true);
            assert_eq!(worker.next_poll, u64::MAX);
        }
    }
    assert_eq!(
        command(&mut worker, json!({"id":4,"op":"defaults"}))["ok"],
        false
    );
    assert_eq!(
        command(
            &mut worker,
            json!({"id":5,"op":"set","key":"gain","value":40})
        )["ok"],
        false
    );
    let fake = shared.borrow();
    assert_eq!(fake.attached_checks, 1);
    assert_eq!(fake.present_checks, 0);
    assert_eq!(fake.opens, 0);
    assert_eq!(fake.reads, 0);
    assert_eq!(fake.defaults_calls, 0);
    assert!(fake.alsa_calls.is_empty());
    assert!(fake.writes.is_empty());
}

#[test]
fn attach_event_resumes_sampling_immediately_and_regular_ticks_do_not_scan_inventory() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().attached = false;
    worker.sample(false);
    worker.take_events();
    shared.borrow_mut().attached = true;
    now.set(2000);
    worker.tick();
    assert_eq!(
        shared.borrow().opens,
        0,
        "inventory changes must be event driven"
    );
    worker.take_events();

    worker.hardware_changed();
    let wake = worker.take_events();
    assert_eq!(wake.len(), 1);
    assert_eq!(wake[0]["state"]["present"], true);
    assert_eq!(wake[0]["state"]["stale"], true);
    assert_eq!(
        shared.borrow().opens,
        0,
        "watchdog wake notice must precede USB work"
    );
    assert_eq!(worker.wait_ms(), Some(0));
    worker.tick();

    assert_eq!(shared.borrow().attached_checks, 2);
    assert_eq!(shared.borrow().opens, 1);
    assert_eq!(shared.borrow().reads, 1);
    assert_eq!(worker.state["connected"], true);
    assert_eq!(worker.state["present"], true);
    assert_eq!(worker.next_poll, 2500);
    let events = worker.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["sampledAtMs"], 2000);
    for instant in [2500, 3000, 3500] {
        now.set(instant);
        worker.tick();
    }
    let fake = shared.borrow();
    assert_eq!(fake.attached_checks, 2);
    assert_eq!(fake.opens, 1);
    assert_eq!(fake.reads, 4);
    assert!(fake.writes.is_empty());
}

#[test]
fn detach_event_releases_usb_and_stops_all_device_probes_until_reattachment() {
    let (mut worker, now, shared) = fixture();
    worker.sample(false);
    worker.take_events();
    now.set(1100);
    shared.borrow_mut().attached = false;
    worker.hardware_changed();
    assert_eq!(worker.next_poll, u64::MAX);
    assert_eq!(shared.borrow().drops, 1);
    assert_eq!(
        worker.state,
        json!({"connected":false,"usb":false,"stale":false,"present":false})
    );
    let events = worker.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["sampledAtMs"], 1000);
    now.set(10000);
    worker.tick();
    assert_eq!(
        command(&mut worker, json!({"id":1,"op":"refresh"}))["state"]["present"],
        false
    );
    assert_eq!(
        command(
            &mut worker,
            json!({"id":2,"op":"set","key":"gain","value":40})
        )["ok"],
        false
    );
    {
        let fake = shared.borrow();
        assert_eq!(fake.attached_checks, 2);
        assert_eq!(fake.opens, 1);
        assert_eq!(fake.reads, 1);
        assert_eq!(fake.present_checks, 0);
        assert!(fake.alsa_calls.is_empty());
        assert!(fake.writes.is_empty());
    }
    shared.borrow_mut().attached = true;
    worker.hardware_changed();
    worker.tick();
    assert_eq!(worker.state["connected"], true);
    assert_eq!(worker.state["gain"], 43);
    assert_eq!(shared.borrow().opens, 2);
    assert!(
        shared.borrow().writes.is_empty(),
        "disconnected edits must not replay"
    );
}

#[test]
fn busy_attached_device_retries_without_inventory_scans_and_detach_cancels_retry() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Interface busy".into(), false));
    worker.sample(false);
    assert_eq!(worker.next_poll, 1250);
    now.set(1249);
    worker.tick();
    assert_eq!(shared.borrow().opens, 1);
    now.set(1250);
    worker.tick();
    assert_eq!(shared.borrow().opens, 2);
    assert_eq!(shared.borrow().attached_checks, 1);
    assert_eq!(worker.next_poll, 1750);
    shared.borrow_mut().attached = false;
    worker.hardware_changed();
    assert_eq!(worker.next_poll, u64::MAX);
    assert_eq!(worker.state["stale"], false);
    assert!(worker.state.get("error").is_none());
    now.set(10000);
    worker.tick();
    let fake = shared.borrow();
    assert_eq!(fake.opens, 2);
    assert_eq!(fake.attached_checks, 2);
    assert_eq!(fake.present_checks, 0);
    assert!(fake.alsa_calls.is_empty());
}

#[test]
fn detach_event_stops_permission_fallback_without_launching_alsa() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(false);
    assert_eq!(worker.state["connected"], true);
    assert_eq!(shared.borrow().alsa_calls.len(), 1);
    let present_checks = shared.borrow().present_checks;
    shared.borrow_mut().attached = false;
    worker.hardware_changed();
    now.set(10000);
    worker.tick();
    worker.sample(true);
    assert_eq!(worker.state["present"], false);
    assert_eq!(worker.next_poll, u64::MAX);
    let fake = shared.borrow();
    assert_eq!(fake.attached_checks, 2);
    assert_eq!(fake.opens, 1);
    assert_eq!(fake.alsa_calls.len(), 1);
    assert_eq!(fake.present_checks, present_checks);
    assert!(fake.writes.is_empty());
}

#[test]
fn poll_updates_immediately_then_suppresses_unchanged_samples_until_heartbeat() {
    let (mut worker, now, shared) = fixture();
    worker.sample(true);
    let initial = worker.take_events();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0]["state"]["gain"], 43);
    assert_eq!(initial[0]["sampledAtMs"], 1000);
    assert_eq!(worker.wait_ms(), Some(500));
    assert_eq!(
        command(&mut worker, json!({"id":1,"op":"poll","intervalMs":50}))["ok"],
        true
    );
    assert_eq!(worker.next_poll, 1050);
    now.set(1050);
    worker.tick();
    assert!(worker.take_events().is_empty());
    now.set(2000);
    worker.tick();
    let events = worker.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["seq"], 3);
    assert_eq!(events[0]["sampledAtMs"], 2000);
    assert_eq!(shared.borrow().opens, 1);
    assert!(shared.borrow().writes.is_empty());
}

#[test]
fn disconnected_backoff_keeps_liveness_without_extra_device_probes() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Disconnected".into(), false));
    worker.backoff = 5000;
    worker.sample(false);
    assert_eq!(worker.next_poll, 6000);
    assert_eq!(worker.wait_ms(), Some(1000));
    worker.take_events();
    for instant in [2000, 3000, 4000, 5000] {
        now.set(instant);
        worker.tick();
        let events = worker.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["state"]["stale"], true);
        assert_eq!(events[0]["sampledAtMs"], 0);
        assert_eq!(worker.wait_ms(), Some(1000));
        assert_eq!(shared.borrow().opens, 1);
    }
    now.set(6000);
    worker.tick();
    assert_eq!(shared.borrow().opens, 2);
    assert_eq!(worker.next_poll, 11000);
}

#[test]
fn reconnect_backoff_doubles_caps_and_resets_after_a_successful_read() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Disconnected".into(), false));
    for delay in [250, 500, 1000, 2000, 4000, 5000, 5000] {
        worker.sample(false);
        assert_eq!(worker.next_connect, now.get() + delay);
        let opens = shared.borrow().opens;
        now.set(worker.next_connect - 1);
        worker.sample(false);
        assert_eq!(
            shared.borrow().opens,
            opens,
            "must not probe before backoff expires"
        );
        now.set(worker.next_connect);
    }
    shared.borrow_mut().open_error = None;
    worker.sample(false);
    assert_eq!(worker.state["connected"], true);
    assert_eq!(worker.backoff, 250);
}

#[test]
fn fallback_requires_permission_error_and_a_present_card() {
    for (permission, present, expected) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let (mut worker, _, shared) = fixture();
        {
            let mut fake = shared.borrow_mut();
            fake.open_error = Some((
                if permission {
                    "Permission denied"
                } else {
                    "Interface busy"
                }
                .into(),
                permission,
            ));
            fake.present = present;
        }
        worker.sample(false);
        assert_eq!(worker.state["connected"], expected);
        assert_eq!(shared.borrow().alsa_calls.len(), usize::from(expected));
        assert_eq!(worker.state["usb"], false);
    }
}

#[test]
fn fallback_polls_slowly_and_does_not_flicker_during_permission_retries() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(false);
    assert_eq!(worker.next_poll, 3000);
    assert_eq!(worker.wait_ms(), Some(1000));
    worker.take_events();
    now.set(2000);
    worker.tick();
    assert_eq!(
        shared.borrow().alsa_calls.len(),
        1,
        "heartbeat must not launch ALSA"
    );
    now.set(3000);
    worker.tick();
    let events = worker.take_events();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["state"]["connected"] == true));
    assert_eq!(shared.borrow().opens, 2);
    assert_eq!(shared.borrow().alsa_calls.len(), 2);
    assert_eq!(
        command(&mut worker, json!({"id":1,"op":"poll","intervalMs":50}))["ok"],
        true
    );
    assert_eq!(worker.next_poll, 3500);
}

#[test]
fn fallback_disappearance_marks_stale_before_vendor_retry_is_due() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(false);
    assert_eq!(worker.state["connected"], true);
    worker.take_events();
    worker.next_connect = 9000;
    now.set(1500);
    shared.borrow_mut().present = false;

    worker.sample(false);

    assert_eq!(worker.state["connected"], false);
    assert_eq!(worker.state["stale"], true);
    assert_eq!(worker.state["gain"], 43);
    assert_eq!(worker.state["headphones"], -20.0);
    let events = worker.take_events();
    assert_eq!(events.len(), 1, "disconnection must publish immediately");
    assert_eq!(events[0]["state"]["connected"], false);
    assert_eq!(events[0]["sampledAtMs"], 1000);
    let fake = shared.borrow();
    assert_eq!(
        fake.opens, 1,
        "vendor retry must not be needed to notice removal"
    );
    assert_eq!(
        fake.alsa_calls.len(),
        1,
        "missing card must not launch ALSA"
    );
    assert!(fake.writes.is_empty());
}

#[test]
fn fallback_rejected_write_publishes_fresh_state_and_is_never_replayed() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(false);
    worker.take_events();
    {
        let mut fake = shared.borrow_mut();
        fake.ignore_write = true;
        fake.alsa_state["gain"] = json!(42); // Physical update after the last UI sample.
    }
    now.set(1500);
    worker.command(json!({"id":1,"op":"set","key":"gain","value":41}));
    let events = worker.take_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "state");
    assert_eq!(events[0]["sampledAtMs"], 1500);
    assert_eq!(events[1]["ok"], false);
    assert!(events[1]["error"].as_str().unwrap().contains("confirm"));
    assert_eq!(events[1]["state"]["gain"], 42);
    assert_eq!(events[1]["state"]["connected"], true);
    assert_eq!(events[1]["state"]["stale"], false);

    now.set(worker.next_poll);
    worker.tick();
    let fake = shared.borrow();
    assert_eq!(
        fake.alsa_calls.iter().filter(|edit| edit.is_some()).count(),
        1
    );
    assert_eq!(worker.state["gain"], 42);
    assert!(fake.writes.is_empty());
}

#[test]
fn busy_interface_revokes_previous_fallback() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(false);
    shared.borrow_mut().open_error = Some(("Interface busy".into(), false));
    now.set(worker.next_poll);
    worker.tick();
    assert_eq!(worker.state["connected"], false);
    assert_eq!(worker.state["stale"], true);
    assert_eq!(shared.borrow().alsa_calls.len(), 1);
}

#[test]
fn physical_changes_publish_without_writing() {
    let (mut worker, now, shared) = fixture();
    worker.sample(true);
    worker.take_events();
    shared.borrow_mut().blocks.settings[0] = 46;
    now.set(1500);
    worker.tick();
    let events = worker.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["state"]["gain"], 46);
    assert!(shared.borrow().writes.is_empty());
}

#[test]
fn set_uses_fresh_read_and_reports_concurrent_readback_field() {
    let (mut worker, _, shared) = fixture();
    worker.sample(true);
    worker.take_events();
    {
        let mut fake = shared.borrow_mut();
        fake.blocks.settings[1] = 2; // Phantom changed since the last displayed sample.
        fake.blocks.settings[37] = 0xa5; // Unknown byte must survive the full-block write.
        fake.concurrent_strength = Some(12);
    }
    worker.command(json!({"id":"set-1","op":"set","key":"gain","value":41}));
    let events = worker.take_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "state");
    assert_eq!(events[1]["type"], "result");
    assert_eq!(events[1]["id"], "set-1");
    assert_eq!(events[1]["ok"], true);
    assert_eq!(events[1]["state"]["gain"], 41);
    assert_eq!(events[1]["state"]["strength"], 12);
    assert_eq!(events[1]["state"]["phantom"], true);
    let fake = shared.borrow();
    assert_eq!(fake.reads, 3, "set must read before and after writing");
    assert_eq!(fake.writes.len(), 1);
    assert_eq!(fake.writes[0].0, 4);
    assert_eq!(fake.writes[0].1[37], 0xa5);
}

#[test]
fn rejected_readback_reports_actual_state_without_disconnect_or_retry() {
    let (mut worker, now, shared) = fixture();
    worker.sample(true);
    shared.borrow_mut().ignore_write = true;
    let result = command(
        &mut worker,
        json!({"id":1,"op":"set","key":"gain","value":41}),
    );
    assert_eq!(result["ok"], false);
    assert!(result["error"].as_str().unwrap().contains("confirm"));
    assert_eq!(result["state"]["gain"], 43);
    assert_eq!(result["state"]["connected"], true);
    assert_eq!(result["state"]["stale"], false);
    now.set(1500);
    worker.tick();
    assert_eq!(shared.borrow().writes.len(), 1);
    assert_eq!(shared.borrow().drops, 0);
}

#[test]
fn failed_write_keeps_stale_values_releases_handle_and_is_never_replayed() {
    let (mut worker, now, shared) = fixture();
    worker.sample(true);
    shared.borrow_mut().write_error = true;
    let result = command(
        &mut worker,
        json!({"id":1,"op":"set","key":"gain","value":40}),
    );
    assert_eq!(result["ok"], false);
    assert_eq!(result["state"]["gain"], 43);
    assert_eq!(result["state"]["connected"], false);
    assert_eq!(result["state"]["stale"], true);
    assert_eq!(shared.borrow().drops, 1);
    assert_eq!(
        command(
            &mut worker,
            json!({"id":2,"op":"set","key":"gain","value":39})
        )["ok"],
        false
    );
    shared.borrow_mut().write_error = false;
    now.set(1500);
    worker.tick();
    assert_eq!(worker.state["connected"], true);
    assert_eq!(worker.state["gain"], 43);
    assert_eq!(shared.borrow().opens, 2);
    assert_eq!(
        shared.borrow().writes.len(),
        1,
        "failed commands must not be replayed"
    );
}

#[test]
fn invalid_commands_have_no_hardware_effect_and_do_not_disconnect() {
    let (mut worker, _, shared) = fixture();
    worker.sample(true);
    for message in [
        json!(null),
        json!([]),
        json!({"op":"refresh"}),
        json!({"id":true,"op":"refresh"}),
        json!({"id":1.5,"op":"refresh"}),
        json!({"id":1,"op":"unknown"}),
        json!({"id":1,"op":"poll","intervalMs":1}),
        json!({"id":1,"op":"poll","intervalMs":true}),
        json!({"id":1,"op":"set","key":"nope","value":1}),
        json!({"id":1,"op":"set","key":"gain","value":-1}),
        json!({"id":1,"op":"set","key":"gain","value":81}),
        json!({"id":1,"op":"set","key":"gain","value":"12"}),
        json!({"id":1,"op":"set","key":"gain","value":null}),
        json!({"id":1,"op":"set","key":"mute","value":2}),
    ] {
        assert_eq!(
            command(&mut worker, message.clone())["ok"],
            false,
            "{message}"
        );
    }
    let fake = shared.borrow();
    assert_eq!(fake.reads, 1);
    assert_eq!(fake.opens, 1);
    assert!(fake.writes.is_empty());
    assert_eq!(worker.interval, 500);
    assert_eq!(worker.state["connected"], true);
    assert_eq!(worker.state["stale"], false);
}

#[test]
fn invalid_fresh_device_state_blocks_write_and_preserves_last_good_sample() {
    let (mut worker, _, shared) = fixture();
    worker.sample(true);
    shared.borrow_mut().blocks.settings[0] = 255;
    let result = command(
        &mut worker,
        json!({"id":1,"op":"set","key":"gain","value":40}),
    );
    assert_eq!(result["ok"], false);
    assert_eq!(result["state"]["gain"], 43);
    assert_eq!(result["state"]["stale"], true);
    assert_eq!(result["state"]["connected"], false);
    assert!(shared.borrow().writes.is_empty());
    assert_eq!(shared.borrow().drops, 1);
}

#[test]
fn fallback_accepts_basic_controls_and_rejects_dsp_without_calls() {
    let (mut worker, _, shared) = fixture();
    shared.borrow_mut().open_error = Some(("Permission denied".into(), true));
    worker.sample(true);
    for (key, value, expected) in [
        ("gain", 40.8, json!(40)),
        ("headphones", -20.25, json!(-20.25)),
        ("mute", 1.0, json!(true)),
    ] {
        let result = command(
            &mut worker,
            json!({"id":1,"op":"set","key":key,"value":value}),
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["state"][key], expected);
    }
    let calls = shared.borrow().alsa_calls.len();
    let result = command(
        &mut worker,
        json!({"id":1,"op":"set","key":"phantom","value":1}),
    );
    assert_eq!(result["ok"], false);
    assert_eq!(shared.borrow().alsa_calls.len(), calls);
    assert_eq!(worker.state["connected"], true);
}

#[test]
fn defaults_run_once_when_connected_and_preserve_device_on_routing_failure() {
    let (mut worker, _, shared) = fixture();
    assert_eq!(
        command(&mut worker, json!({"id":1,"op":"defaults"}))["ok"],
        false
    );
    assert_eq!(shared.borrow().defaults_calls, 0);
    worker.sample(true);
    let before = worker.state.clone();
    assert_eq!(
        command(&mut worker, json!({"id":2,"op":"defaults"}))["ok"],
        true
    );
    assert_eq!(shared.borrow().defaults_calls, 1);
    shared.borrow_mut().defaults_error = true;
    assert_eq!(
        command(&mut worker, json!({"id":3,"op":"defaults"}))["ok"],
        false
    );
    assert_eq!(shared.borrow().defaults_calls, 2);
    assert_eq!(worker.state, before);
    assert_eq!(shared.borrow().reads, 1);
    assert!(shared.borrow().writes.is_empty());
}

#[test]
fn read_failure_heartbeats_retain_last_successful_sample_time() {
    let (mut worker, now, shared) = fixture();
    worker.sample(true);
    worker.take_events();
    shared.borrow_mut().read_error = true;
    shared.borrow_mut().open_error = Some(("Disconnected".into(), false));
    worker.backoff = 5000;
    now.set(1500);
    worker.tick();
    let failed = worker.take_events();
    assert_eq!(failed[0]["state"]["gain"], 43);
    assert_eq!(failed[0]["sampledAtMs"], 1000);
    now.set(2500);
    worker.tick();
    let heartbeat = worker.take_events();
    assert_eq!(heartbeat.len(), 1);
    assert_eq!(heartbeat[0]["sampledAtMs"], 1000);
    assert_eq!(shared.borrow().opens, 1);
    assert_eq!(shared.borrow().drops, 1);
}

#[test]
fn failed_attach_is_present_and_keeps_timed_recovery_after_indefinite_sleep() {
    let (mut worker, now, shared) = fixture();
    shared.borrow_mut().attached = false;
    worker.sample(false);
    assert_eq!(worker.wait_ms(), None);
    worker.take_events();
    shared.borrow_mut().attached = true;
    shared.borrow_mut().open_error = Some(("Interface busy".into(), false));
    worker.hardware_changed();
    let waking = worker.take_events();
    assert_eq!(waking[0]["state"]["present"], true);
    assert_eq!(shared.borrow().opens, 0);
    worker.tick();
    assert_eq!(worker.state["present"], true);
    assert_eq!(worker.state["stale"], true);
    assert_eq!(worker.wait_ms(), Some(250));
    now.set(1250);
    worker.tick();
    assert_eq!(shared.borrow().opens, 2);
}
