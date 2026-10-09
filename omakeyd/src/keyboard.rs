//! The virtual keyboard. Keys are reference-counted across sessions so two
//! phones holding the same key don't release it for each other.

use crate::protocol::{LED_CAPS, LED_NUM, LED_SCROLL};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    /// The lock LEDs the compositor set (protocol LED_* bits), or None when
    /// this sink can't tell.
    fn leds(&mut self) -> Option<u8> {
        None
    }
}

/// Two virtual devices: a keyboard, and a mouse for the phone's touchpad.
/// Keeping them apart stops the compositor treating the keyboard as a pointer.
pub struct Uinput {
    keyboard: RawKeyboard,
    mouse: evdev::uinput::VirtualDevice,
    /// Hi-res scroll not yet sent as whole notches, for apps that only read REL_WHEEL.
    wheel_rest: i32,
    hwheel_rest: i32,
}

impl Uinput {
    pub fn open() -> io::Result<Uinput> {
        let keyboard = RawKeyboard::open()?;

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

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_LED: u16 = 0x11;

/// `_IOC` in the generic Linux encoding (x86, ARM, RISC-V).
const fn ioc(write: bool, nr: u8, size: usize) -> libc::Ioctl {
    (((write as u32) << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr as u32) as libc::Ioctl
}
const UI_DEV_CREATE: libc::Ioctl = ioc(false, 1, 0);
const UI_DEV_DESTROY: libc::Ioctl = ioc(false, 2, 0);
const UI_DEV_SETUP: libc::Ioctl = ioc(true, 3, std::mem::size_of::<libc::uinput_setup>());
const UI_SET_EVBIT: libc::Ioctl = ioc(true, 100, 4);
const UI_SET_KEYBIT: libc::Ioctl = ioc(true, 101, 4);
const UI_SET_LEDBIT: libc::Ioctl = ioc(true, 105, 4);

/// "Omakey Keyboard", made with raw uinput ioctls because evdev's builder
/// can't declare LEDs. With Num/Caps/Scroll Lock LEDs the compositor tells
/// us the lock state, which the phone shows.
struct RawKeyboard {
    fd: OwnedFd,
    leds: u8,
}

impl RawKeyboard {
    fn open() -> io::Result<RawKeyboard> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/uinput")?;
        let fd = OwnedFd::from(file);
        let raw = fd.as_raw_fd();
        let set = |req: libc::Ioctl, v: u16| -> io::Result<()> {
            // SAFETY: these ioctls take an int by value.
            if unsafe { libc::ioctl(raw, req, v as libc::c_int) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        };
        set(UI_SET_EVBIT, EV_KEY)?;
        for code in 1..=KEY_MAX {
            if is_key(code) {
                set(UI_SET_KEYBIT, code)?;
            }
        }
        set(UI_SET_EVBIT, EV_LED)?;
        for led in 0..3 {
            set(UI_SET_LEDBIT, led)?;
        }
        // SAFETY: plain C struct, all zeroes is valid.
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        // BUS_VIRTUAL; vendor/product are arbitrary but stable so the
        // compositor's per-device config can match them.
        setup.id = libc::input_id { bustype: 0x06, vendor: 0x4f4b, product: 0x0001, version: 1 };
        for (d, s) in setup.name.iter_mut().zip(b"Omakey Keyboard") {
            *d = *s as libc::c_char;
        }
        // SAFETY: UI_DEV_SETUP reads one uinput_setup; UI_DEV_CREATE takes no argument.
        if unsafe { libc::ioctl(raw, UI_DEV_SETUP, &setup as *const libc::uinput_setup) } < 0
            || unsafe { libc::ioctl(raw, UI_DEV_CREATE) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(RawKeyboard { fd, leds: 0 })
    }

    /// One key change and its SYN_REPORT, in one write.
    fn emit_key(&mut self, code: u16, down: bool) -> io::Result<()> {
        let ev = |kind: u16, code: u16, value: i32| libc::input_event {
            time: libc::timeval { tv_sec: 0, tv_usec: 0 },
            type_: kind,
            code,
            value,
        };
        let evs = [ev(EV_KEY, code, down as i32), ev(EV_SYN, 0, 0)];
        let len = std::mem::size_of_val(&evs);
        // SAFETY: writes `len` bytes from a live array.
        let n = unsafe { libc::write(self.fd.as_raw_fd(), evs.as_ptr().cast(), len) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize != len {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short uinput write"));
        }
        Ok(())
    }

    /// Drain the LED events the compositor wrote to our device. Non-blocking.
    fn poll_leds(&mut self) -> u8 {
        // SAFETY: plain C structs, all zeroes is valid.
        let mut evs: [libc::input_event; 16] = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: reads at most the array's size into it.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), evs.as_mut_ptr().cast(), std::mem::size_of_val(&evs)) };
            if n <= 0 {
                break;
            }
            let count = n as usize / std::mem::size_of::<libc::input_event>();
            for e in &evs[..count] {
                // LED_NUML, LED_CAPSL, LED_SCROLLL
                let bit = match (e.type_, e.code) {
                    (EV_LED, 0) => LED_NUM,
                    (EV_LED, 1) => LED_CAPS,
                    (EV_LED, 2) => LED_SCROLL,
                    _ => continue,
                };
                if e.value != 0 {
                    self.leds |= bit;
                } else {
                    self.leds &= !bit;
                }
            }
            if count < evs.len() {
                break;
            }
        }
        self.leds
    }
}

impl Drop for RawKeyboard {
    fn drop(&mut self) {
        // SAFETY: takes no argument; closing the fd would destroy it too.
        unsafe { libc::ioctl(self.fd.as_raw_fd(), UI_DEV_DESTROY) };
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
        // Every key is its own frame, so the compositor sees presses in
        // exactly the order we send them.
        if is_button(code) {
            let ev = evdev::InputEvent::new(evdev::EventType::KEY.0, code, down as i32);
            self.mouse.emit(&[ev])
        } else {
            self.keyboard.emit_key(code, down)
        }
    }

    fn leds(&mut self) -> Option<u8> {
        Some(self.keyboard.poll_leds())
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
    /// None while there's no device: another user's session is in front.
    sink: Option<Box<dyn KeySink + Send>>,
    count: Vec<u8>,
    /// Cleared by the seat watcher the moment another session comes to the
    /// front; nothing is written from then on, before the device goes.
    in_front: Arc<AtomicBool>,
}

impl Keyboard {
    #[cfg(test)]
    pub fn new(sink: Box<dyn KeySink + Send>) -> Keyboard {
        Keyboard::gated(Some(sink), Arc::new(AtomicBool::new(true)))
    }

    /// A keyboard that types only while `in_front` is set.
    pub fn gated(sink: Option<Box<dyn KeySink + Send>>, in_front: Arc<AtomicBool>) -> Keyboard {
        Keyboard { sink, count: vec![0; KEY_MAX as usize + 1], in_front }
    }

    /// The device, if there is one and its keys may reach the seat.
    fn live(&mut self) -> Option<&mut Box<dyn KeySink + Send>> {
        if self.in_front.load(Ordering::SeqCst) {
            self.sink.as_mut()
        } else {
            None
        }
    }

    pub fn in_front(&self) -> bool {
        self.in_front.load(Ordering::SeqCst)
    }

    pub fn has_device(&self) -> bool {
        self.sink.is_some()
    }

    /// Swap the device. Nothing is held on the new one; dropping the old one
    /// removes it, and the kernel lets go of its keys.
    pub fn set_device(&mut self, sink: Option<Box<dyn KeySink + Send>>) {
        self.sink = sink;
        self.count.fill(0);
    }

    /// Current lock LEDs (protocol LED_* bits), when the device reports them.
    pub fn leds(&mut self) -> Option<u8> {
        self.live()?.leds()
    }

    pub fn press(&mut self, code: u16) {
        let down = self.count[code as usize] > 0;
        if !down {
            let Some(sink) = self.live() else { return };
            if let Err(e) = sink.key(code, true) {
                eprintln!("omakeyd: uinput write failed: {e}");
            }
        }
        let c = &mut self.count[code as usize];
        *c = c.saturating_add(1);
    }

    pub fn release(&mut self, code: u16) {
        let c = &mut self.count[code as usize];
        if *c == 0 {
            return;
        }
        *c -= 1;
        if *c == 0 {
            if let Some(sink) = self.live() {
                if let Err(e) = sink.key(code, false) {
                    eprintln!("omakeyd: uinput write failed: {e}");
                }
            }
        }
    }

    pub fn pointer(&mut self, p: crate::protocol::Pointer) {
        if let Some(sink) = self.live() {
            if let Err(e) = sink.pointer(p) {
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

    /// Keys, pointer motion, and the LED state to report.
    #[derive(Clone, Default)]
    pub struct Recorder(
        pub Arc<Mutex<Vec<(u16, bool)>>>,
        pub Arc<Mutex<Vec<crate::protocol::Pointer>>>,
        pub Arc<Mutex<Option<u8>>>,
    );

    impl KeySink for Recorder {
        fn key(&mut self, code: u16, down: bool) -> io::Result<()> {
            self.0.lock().unwrap().push((code, down));
            Ok(())
        }
        fn pointer(&mut self, p: crate::protocol::Pointer) -> io::Result<()> {
            self.1.lock().unwrap().push(p);
            Ok(())
        }
        fn leds(&mut self) -> Option<u8> {
            *self.2.lock().unwrap()
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
