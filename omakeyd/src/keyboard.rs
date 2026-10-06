//! The virtual keyboard. Keys are reference-counted across sessions so two
//! phones holding the same key don't release it for each other.

use std::io;

pub const KEY_MAX: u16 = 0x2ff;

pub const BTN_LEFT: u16 = 0x110;
#[allow(dead_code)] // the range is BTN_LEFT..=BTN_MIDDLE; named for readers and tests
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

/// Keyboard codes. 0x100-0x15f and 0x2c0+ are mouse and joystick buttons,
/// which would make the compositor see the keyboard as a pointer too.
pub fn is_key(code: u16) -> bool {
    (1..=0xff).contains(&code) || (0x160..=0x2bf).contains(&code)
}

/// The touchpad's buttons; they go to a separate virtual mouse.
pub fn is_button(code: u16) -> bool {
    (BTN_LEFT..=BTN_MIDDLE).contains(&code)
}

/// Codes a phone may send.
pub fn allowed(code: u16) -> bool {
    is_key(code) || is_button(code)
}

pub fn is_modifier(code: u16) -> bool {
    // LEFTCTRL LEFTSHIFT RIGHTSHIFT LEFTALT RIGHTCTRL RIGHTALT LEFTMETA RIGHTMETA FN
    matches!(code, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126 | 464)
}

pub trait KeySink {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()>;
    /// Relative motion and scroll (1/120 notch units) from the touchpad.
    fn pointer(&mut self, _p: crate::protocol::Pointer) -> io::Result<()> {
        Ok(())
    }
}

/// Two virtual devices: a keyboard, and a mouse for the phone's touchpad.
/// Keeping them apart stops the compositor treating the keyboard as a pointer.
pub struct Uinput {
    keyboard: evdev::uinput::VirtualDevice,
    mouse: evdev::uinput::VirtualDevice,
    /// Hi-res scroll not yet sent as whole notches, for apps that only read REL_WHEEL.
    wheel_rest: i32,
    hwheel_rest: i32,
}

impl Uinput {
    pub fn open() -> io::Result<Uinput> {
        let mut keys = evdev::AttributeSet::<evdev::KeyCode>::new();
        for code in 1..=KEY_MAX {
            if is_key(code) {
                keys.insert(evdev::KeyCode::new(code));
            }
        }
        let keyboard = evdev::uinput::VirtualDevice::builder()?
            .name("Omakey Keyboard")
            // BUS_VIRTUAL; vendor/product are arbitrary but stable so the
            // compositor's per-device config can match them.
            .input_id(evdev::InputId::new(evdev::BusType::BUS_VIRTUAL, 0x4f4b, 0x0001, 1))
            .with_keys(&keys)?
            .build()?;

        let mut buttons = evdev::AttributeSet::<evdev::KeyCode>::new();
        for code in BTN_LEFT..=BTN_MIDDLE {
            buttons.insert(evdev::KeyCode::new(code));
        }
        let mut axes = evdev::AttributeSet::<evdev::RelativeAxisCode>::new();
        for a in [
            evdev::RelativeAxisCode::REL_X,
            evdev::RelativeAxisCode::REL_Y,
            evdev::RelativeAxisCode::REL_WHEEL,
            evdev::RelativeAxisCode::REL_HWHEEL,
            evdev::RelativeAxisCode::REL_WHEEL_HI_RES,
            evdev::RelativeAxisCode::REL_HWHEEL_HI_RES,
        ] {
            axes.insert(a);
        }
        let mouse = evdev::uinput::VirtualDevice::builder()?
            .name("Omakey Mouse")
            .input_id(evdev::InputId::new(evdev::BusType::BUS_VIRTUAL, 0x4f4b, 0x0002, 1))
            .with_keys(&buttons)?
            .with_relative_axes(&axes)?
            .build()?;
        Ok(Uinput { keyboard, mouse, wheel_rest: 0, hwheel_rest: 0 })
    }
}

/// Whole notches in `rest` + `delta` (1/120 units), keeping the remainder.
fn notches(rest: &mut i32, delta: i16) -> i32 {
    *rest += delta as i32;
    let n = *rest / 120;
    *rest -= n * 120;
    n
}

impl KeySink for Uinput {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
        let ev = evdev::InputEvent::new(evdev::EventType::KEY.0, code, down as i32);
        // emit() appends SYN_REPORT, so every key is its own frame and the
        // compositor sees presses in exactly the order we send them.
        if is_button(code) {
            self.mouse.emit(&[ev])
        } else {
            self.keyboard.emit(&[ev])
        }
    }

    fn pointer(&mut self, p: crate::protocol::Pointer) -> io::Result<()> {
        use evdev::RelativeAxisCode as R;
        let rel = |axis: R, v: i32| evdev::InputEvent::new(evdev::EventType::RELATIVE.0, axis.0, v);
        let mut evs = Vec::with_capacity(6);
        if p.dx != 0 {
            evs.push(rel(R::REL_X, p.dx as i32));
        }
        if p.dy != 0 {
            evs.push(rel(R::REL_Y, p.dy as i32));
        }
        if p.wheel != 0 {
            evs.push(rel(R::REL_WHEEL_HI_RES, p.wheel as i32));
            let n = notches(&mut self.wheel_rest, p.wheel);
            if n != 0 {
                evs.push(rel(R::REL_WHEEL, n));
            }
        }
        if p.hwheel != 0 {
            evs.push(rel(R::REL_HWHEEL_HI_RES, p.hwheel as i32));
            let n = notches(&mut self.hwheel_rest, p.hwheel);
            if n != 0 {
                evs.push(rel(R::REL_HWHEEL, n));
            }
        }
        if evs.is_empty() {
            return Ok(());
        }
        // One frame: motion and scroll of one packet arrive together.
        self.mouse.emit(&evs)
    }
}

/// Prints key events instead of typing them (`--dry-run`).
pub struct LogSink;

impl KeySink for LogSink {
    fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
        println!("key {code} {}", if down { "down" } else { "up" });
        Ok(())
    }

    fn pointer(&mut self, p: crate::protocol::Pointer) -> io::Result<()> {
        println!("pointer {} {} wheel {} {}", p.dx, p.dy, p.wheel, p.hwheel);
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

    pub fn pointer(&mut self, p: crate::protocol::Pointer) {
        if let Err(e) = self.sink.pointer(p) {
            eprintln!("omakeyd: uinput write failed: {e}");
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
    pub struct Recorder(pub Arc<Mutex<Vec<(u16, bool)>>>, pub Arc<Mutex<Vec<crate::protocol::Pointer>>>);

    impl KeySink for Recorder {
        fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
            self.0.lock().unwrap().push((code, down));
            Ok(())
        }
        fn pointer(&mut self, p: crate::protocol::Pointer) -> io::Result<()> {
            self.1.lock().unwrap().push(p);
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
    fn allowed_codes_are_keys_and_the_three_mouse_buttons() {
        assert!(allowed(1) && allowed(125) && allowed(464));
        assert!(allowed(BTN_LEFT) && allowed(BTN_RIGHT) && allowed(BTN_MIDDLE));
        assert!(!is_key(BTN_LEFT) && is_button(BTN_MIDDLE));
        assert!(!allowed(0) && !allowed(0x113) && !allowed(0x100) && !allowed(0x2c0) && !allowed(0x300));
    }

    #[test]
    fn hi_res_scroll_becomes_whole_notches_with_remainder() {
        let mut rest = 0;
        assert_eq!(notches(&mut rest, 60), 0);
        assert_eq!(notches(&mut rest, 70), 1);
        assert_eq!(rest, 10);
        assert_eq!(notches(&mut rest, -250), -2);
        assert_eq!(rest, 0);
    }
}
