//! The virtual keyboard. Keys are reference-counted across sessions so two
//! phones holding the same key don't release it for each other.

use std::io;

pub const KEY_MAX: u16 = 0x2ff;

/// Codes the device registers and accepts. 0x100-0x15f and 0x2c0+ are mouse
/// and joystick buttons, which would make the compositor see a pointer.
pub fn allowed(code: u16) -> bool {
    (1..=0xff).contains(&code) || (0x160..=0x2bf).contains(&code)
}

pub fn is_modifier(code: u16) -> bool {
    // LEFTCTRL LEFTSHIFT RIGHTSHIFT LEFTALT RIGHTCTRL RIGHTALT LEFTMETA RIGHTMETA FN
    matches!(code, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126 | 464)
}

pub trait KeySink {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()>;
}

pub struct Uinput {
    dev: evdev::uinput::VirtualDevice,
}

impl Uinput {
    pub fn open() -> io::Result<Uinput> {
        let mut keys = evdev::AttributeSet::<evdev::KeyCode>::new();
        for code in 1..=KEY_MAX {
            if allowed(code) {
                keys.insert(evdev::KeyCode::new(code));
            }
        }
        let dev = evdev::uinput::VirtualDevice::builder()?
            .name("Omakey Keyboard")
            // BUS_VIRTUAL; vendor/product are arbitrary but stable so the
            // compositor's per-device config can match them.
            .input_id(evdev::InputId::new(evdev::BusType::BUS_VIRTUAL, 0x4f4b, 0x0001, 1))
            .with_keys(&keys)?
            .build()?;
        Ok(Uinput { dev })
    }
}

impl KeySink for Uinput {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
        let ev = evdev::InputEvent::new(evdev::EventType::KEY.0, code, down as i32);
        // emit() appends SYN_REPORT, so every key is its own frame and the
        // compositor sees presses in exactly the order we send them.
        self.dev.emit(&[ev])
    }
}

/// Prints key events instead of typing them (`--dry-run`).
pub struct LogSink;

impl KeySink for LogSink {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
        println!("key {code} {}", if down { "down" } else { "up" });
        Ok(())
    }
}

pub struct Keyboard {
    sink: Box<dyn KeySink + Send>,
    count: Vec<u8>,
}

impl Keyboard {
    pub fn new(sink: Box<dyn KeySink + Send>) -> Keyboard {
        Keyboard { sink, count: vec![0; KEY_MAX as usize + 1] }
    }

    pub fn press(&mut self, code: u16) {
        let c = &mut self.count[code as usize];
        if *c == 0 {
            if let Err(e) = self.sink.key(code, true) {
                eprintln!("omakeyd: uinput write failed: {e}");
            }
        }
        *c = c.saturating_add(1);
    }

    pub fn release(&mut self, code: u16) {
        let c = &mut self.count[code as usize];
        if *c == 0 {
            return;
        }
        *c -= 1;
        if *c == 0 {
            if let Err(e) = self.sink.key(code, false) {
                eprintln!("omakeyd: uinput write failed: {e}");
            }
        }
    }

    #[cfg(test)]
    pub fn is_down(&self, code: u16) -> bool {
        self.count[code as usize] > 0
    }
}

#[cfg(test)]
pub mod test_sink {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct Recorder(pub Arc<Mutex<Vec<(u16, bool)>>>);

    impl KeySink for Recorder {
        fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
            self.0.lock().unwrap().push((code, down));
            Ok(())
        }
    }

    impl Recorder {
        pub fn take(&self) -> Vec<(u16, bool)> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[test]
    fn refcount_shares_keys() {
        let r = Recorder::default();
        let mut k = Keyboard::new(Box::new(r.clone()));
        k.press(30);
        k.press(30);
        k.release(30);
        assert_eq!(r.take(), vec![(30, true)]);
        k.release(30);
        k.release(30);
        assert_eq!(r.take(), vec![(30, false)]);
    }

    #[test]
    fn allowed_codes_skip_buttons() {
        assert!(allowed(1) && allowed(125) && allowed(464));
        assert!(!allowed(0) && !allowed(0x110) && !allowed(0x2c0) && !allowed(0x300));
    }
}
