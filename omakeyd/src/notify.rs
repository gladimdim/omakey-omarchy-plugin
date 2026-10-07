//! sd_notify(3) without libsystemd: READY=1, WATCHDOG=1 and STOPPING=1
//! datagrams to $NOTIFY_SOCKET, for Type=notify and WatchdogSec=.

use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::{Duration, Instant};

pub struct Notifier {
    sock: Option<(UnixDatagram, SocketAddr)>,
    /// How often to ping the watchdog: a third of its timeout.
    watchdog: Option<Duration>,
    last_ping: Instant,
}

impl Notifier {
    /// From the environment systemd sets; a no-op outside systemd.
    pub fn from_env() -> Notifier {
        let sock = std::env::var_os("NOTIFY_SOCKET").and_then(|path| {
            let b = path.as_bytes();
            let addr = match b.first() {
                Some(b'@') => SocketAddr::from_abstract_name(&b[1..]).ok()?,
                Some(_) => SocketAddr::from_pathname(&path).ok()?,
                None => return None,
            };
            Some((UnixDatagram::unbound().ok()?, addr))
        });
        let ours = std::env::var("WATCHDOG_PID").map_or(true, |p| p.parse() == Ok(std::process::id()));
        let watchdog = std::env::var("WATCHDOG_USEC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|us| *us > 0 && ours)
            .map(|us| Duration::from_micros(us / 3));
        Notifier { sock, watchdog, last_ping: Instant::now() }
    }

    pub fn send(&self, state: &str) {
        if let Some((sock, addr)) = &self.sock {
            if let Err(e) = sock.send_to_addr(state.as_bytes(), addr) {
                eprintln!("omakeyd: sd_notify: {e}");
            }
        }
    }

    /// Call from the main loop; pings the watchdog when it's due.
    pub fn tick(&mut self, now: Instant) {
        if let Some(every) = self.watchdog {
            if now.duration_since(self.last_ping) >= every {
                self.send("WATCHDOG=1");
                self.last_ping = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifies_an_abstract_socket() {
        let name = format!("omakeyd-test-{}", std::process::id());
        let rx = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        let n = Notifier {
            sock: Some((UnixDatagram::unbound().unwrap(), SocketAddr::from_abstract_name(name.as_bytes()).unwrap())),
            watchdog: Some(Duration::from_millis(10)),
            last_ping: Instant::now() - Duration::from_secs(1),
        };
        n.send("READY=1");
        let mut buf = [0u8; 32];
        let len = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"READY=1");
        let mut n = n;
        n.tick(Instant::now());
        let len = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"WATCHDOG=1");
    }
}
