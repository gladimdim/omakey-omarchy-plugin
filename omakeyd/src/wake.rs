//! Wake-on-LAN. While this computer's network card wakes it on a magic
//! packet, phones learn the card's MAC address (WELCOME `wake_mac`, the
//! pairing link's `w`) and send one when the computer doesn't answer.
//!
//! Turning it on needs root, so the daemon only looks. `omakeyd wake-on-lan
//! on` turns it on with sudo, now and at every boot (a udev rule). Over
//! Wi-Fi the card wakes the computer from sleep only; over Ethernet also
//! from power-off, where the firmware allows it.

use crate::daemon;
use crate::server::{Server, Shared};
use serde_json::json;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Re-applies the setting at boot; there only while it's on.
pub const RULE: &str = "/etc/udev/rules.d/71-omakey-wake-on-lan.rules";
/// The card or network can change under us; `wake-on-lan on|off` asks
/// for a look right away.
const CHECK_EVERY: Duration = Duration::from_secs(10);
static RECHECK: AtomicBool = AtomicBool::new(false);

const ETHTOOL_GWOL: u32 = 5;
const WAKE_MAGIC: u32 = 1 << 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeStatus {
    /// The LAN interface a phone can wake, e.g. "wlo1".
    pub interface: Option<String>,
    /// "wifi" or "ethernet"; "" without an interface.
    pub kind: &'static str,
    pub mac: Option<[u8; 6]>,
    /// The card can wake on a magic packet.
    pub supported: bool,
    /// It will: listening for the magic packet, and allowed to wake the computer.
    pub on: bool,
    /// Why not, when it isn't on.
    pub reason: String,
}

impl Default for WakeStatus {
    fn default() -> Self {
        WakeStatus { interface: None, kind: "", mac: None, supported: false, on: false, reason: "starting".into() }
    }
}

impl WakeStatus {
    /// The MAC address phones may wake, while that would work.
    pub fn wake_mac(&self) -> Option<[u8; 6]> {
        self.mac.filter(|_| self.on)
    }
}

pub fn mac_text(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<u8> = s.trim().split(':').map(|p| u8::from_str_radix(p, 16).ok()).collect::<Option<_>>()?;
    let mac: [u8; 6] = parts.try_into().ok()?;
    (mac != [0; 6]).then_some(mac)
}

fn sys(iface: &str) -> PathBuf {
    Path::new("/sys/class/net").join(iface)
}

/// The physical interface with a LAN address, wired first: the card a phone
/// on the same network can wake. Bridges, VPNs and containers have no
/// `device` in sysfs.
fn lan_interface() -> Option<(String, &'static str)> {
    let mut found: Vec<(bool, String)> = Vec::new();
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        let IpAddr::V4(ip) = iface.ip() else { continue };
        let dir = sys(&iface.name);
        if !ip.is_private() || !dir.join("device").exists() {
            continue;
        }
        found.push((dir.join("phy80211").exists(), iface.name));
    }
    found.sort();
    found.into_iter().next().map(|(wifi, name)| (name, if wifi { "wifi" } else { "ethernet" }))
}

fn read_mac(iface: &str) -> Option<[u8; 6]> {
    parse_mac(&std::fs::read_to_string(sys(iface).join("address")).ok()?)
}

fn wifi_phy(iface: &str) -> Option<String> {
    Some(std::fs::read_to_string(sys(iface).join("phy80211/name")).ok()?.trim().to_string())
}

/// Whether the device may wake the computer; None when it has no such switch.
fn device_wakeup(iface: &str) -> Option<bool> {
    Some(std::fs::read_to_string(sys(iface).join("device/power/wakeup")).ok()?.trim() == "enabled")
}

fn iw(args: &[&str]) -> Option<String> {
    let out = Command::new("iw").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `iw phy <phy> wowlan show` says the card wakes on a magic packet.
fn wowlan_magic(show: &str) -> bool {
    show.contains("WoWLAN is enabled") && show.contains("magic packet")
}

/// An Ethernet card's Wake-on-LAN modes, (supported, enabled), as
/// `ethtool <iface>` shows them. Reading them needs no privileges.
fn ethtool_wol(iface: &str) -> Option<(u32, u32)> {
    #[repr(C)]
    struct WolInfo {
        cmd: u32,
        supported: u32,
        wolopts: u32,
        sopass: [u8; 6],
    }
    if iface.len() >= libc::IFNAMSIZ {
        return None;
    }
    let mut wol = WolInfo { cmd: ETHTOOL_GWOL, supported: 0, wolopts: 0, sopass: [0; 6] };
    // SAFETY: plain C struct, all zeroes is valid.
    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    for (d, s) in req.ifr_name.iter_mut().zip(iface.bytes()) {
        *d = s as libc::c_char;
    }
    req.ifr_ifru.ifru_data = (&mut wol as *mut WolInfo).cast();
    // SAFETY: a fresh socket, closed below; `req` points at `wol`, which
    // outlives the call.
    let ok = unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return None;
        }
        let ok = libc::ioctl(fd, libc::SIOCETHTOOL as _, &mut req);
        libc::close(fd);
        ok
    };
    (ok == 0).then_some((wol.supported, wol.wolopts))
}

/// Can `iface` wake on a magic packet, and will it? (supported, listening)
fn magic_packet(iface: &str, kind: &str) -> Result<(bool, bool), String> {
    if kind == "wifi" {
        let phy = wifi_phy(iface).ok_or("can't find its Wi-Fi card")?;
        let info = iw(&["phy", &phy, "info"]).ok_or("iw isn't installed")?;
        let on = iw(&["phy", &phy, "wowlan", "show"]).is_some_and(|s| wowlan_magic(&s));
        Ok((info.contains("wake up on magic packet"), on))
    } else {
        let (supported, enabled) = ethtool_wol(iface).ok_or("its driver doesn't say")?;
        Ok((supported & WAKE_MAGIC != 0, enabled & WAKE_MAGIC != 0))
    }
}

pub fn check() -> WakeStatus {
    let Some((iface, kind)) = lan_interface() else {
        return WakeStatus { reason: "no wired or Wi-Fi network".into(), ..Default::default() };
    };
    let mac = read_mac(&iface);
    let (supported, listening, reason) = match magic_packet(&iface, kind) {
        Ok((false, _)) => (false, false, "this card can't wake on a magic packet".to_string()),
        Ok((true, false)) => (true, false, "off".to_string()),
        Ok((true, true)) if device_wakeup(&iface) == Some(false) => {
            (true, false, "the card isn't allowed to wake the computer".to_string())
        }
        Ok((true, true)) => (true, true, String::new()),
        Err(e) => (false, false, e),
    };
    let on = listening && mac.is_some();
    let reason = if listening && !on { "no MAC address".into() } else { reason };
    WakeStatus { interface: Some(iface), kind, mac, supported, on, reason }
}

/// Keep `server.wake_mac` and `shared.wake` up to date.
pub fn start(server: Arc<Mutex<Server>>, shared: Arc<Mutex<Shared>>) {
    let spawned = std::thread::Builder::new().name("wake".into()).spawn(move || {
        let mut last: Option<WakeStatus> = None;
        loop {
            let status = check();
            if last.as_ref() != Some(&status) {
                match (status.on, &status.interface) {
                    (true, Some(i)) => eprintln!("omakeyd: Wake-on-LAN on for {i}"),
                    _ => eprintln!("omakeyd: Wake-on-LAN off ({})", status.reason),
                }
                server.lock().unwrap().wake_mac = status.wake_mac();
                let mut sh = shared.lock().unwrap();
                sh.wake = status.clone();
                sh.dirty = true;
                drop(sh);
                last = Some(status);
            }
            let until = Instant::now() + CHECK_EVERY;
            while Instant::now() < until && !RECHECK.swap(false, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("omakeyd: can't watch Wake-on-LAN ({e})");
    }
}

/// From the control socket: the setting just changed, look again now.
pub fn recheck() {
    RECHECK.store(true, Ordering::SeqCst);
}

pub fn status_json(s: &WakeStatus) -> serde_json::Value {
    json!({
        "interface": s.interface,
        "kind": s.kind,
        "mac": s.mac.as_ref().map(mac_text),
        "supported": s.supported,
        "on": s.on,
        "reason": s.reason,
    })
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let found = path
        .split(':')
        .chain(["/usr/bin", "/usr/sbin", "/bin", "/sbin"])
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_absolute() && p.is_file());
    found
}

/// The udev rule that turns it on again at every boot.
fn rule_text(kind: &str, mac: &[u8; 6], tool: &Path) -> String {
    let mac = mac_text(mac);
    let tool = tool.display();
    let arm = if kind == "wifi" {
        format!("ACTION==\"add\", SUBSYSTEM==\"ieee80211\", KERNEL==\"phy*\", RUN+=\"{tool} phy %k wowlan enable magic-packet\"")
    } else {
        format!("ACTION==\"add\", SUBSYSTEM==\"net\", ATTR{{address}}==\"{mac}\", RUN+=\"{tool} -s %k wol g\"")
    };
    format!(
        "# Wake-on-LAN for Omakey, so a phone can wake this computer.\n\
         # Written by `omakeyd wake-on-lan on`; `omakeyd wake-on-lan off` removes it.\n\
         {arm}\n\
         ACTION==\"add\", SUBSYSTEM==\"net\", ATTR{{address}}==\"{mac}\", TEST==\"device/power/wakeup\", ATTR{{device/power/wakeup}}=\"enabled\""
    )
}

/// Runs as root: the command that switches the card now, then the
/// device's wakeup switch and the boot rule. Values come in as arguments.
const ROOT_SCRIPT: &str = r#"set -e
wakeup=$1 state=$2 rule=$3 text=$4
shift 4
"$@"
if [ -e "$wakeup" ]; then echo "$state" > "$wakeup"; fi
if [ -n "$text" ]; then printf '%s\n' "$text" > "$rule"; else rm -f "$rule"; fi
udevadm control --reload 2>/dev/null || true
"#;

fn set(on: bool) -> Result<(), String> {
    let (iface, kind) = lan_interface().ok_or("no wired or Wi-Fi network to wake from")?;
    let mac = read_mac(&iface).ok_or_else(|| format!("{iface} has no MAC address"))?;
    let (supported, _) = magic_packet(&iface, kind).unwrap_or((false, false));
    if on && !supported {
        return Err(format!("{iface}'s network card can't wake on a magic packet"));
    }
    let (tool, now): (PathBuf, Vec<String>) = if kind == "wifi" {
        let iw = find_program("iw").ok_or("Wake-on-LAN over Wi-Fi needs iw: sudo pacman -S iw")?;
        let phy = wifi_phy(&iface).ok_or("can't find the Wi-Fi card")?;
        let mode: &[&str] = if on { &["enable", "magic-packet"] } else { &["disable"] };
        let args = ["phy", &phy, "wowlan"].iter().chain(mode).map(|s| s.to_string()).collect();
        (iw, args)
    } else {
        let ethtool = find_program("ethtool").ok_or("Wake-on-LAN over Ethernet needs ethtool: sudo pacman -S ethtool")?;
        (ethtool, vec!["-s".into(), iface.clone(), "wol".into(), if on { "g" } else { "d" }.into()])
    };
    let what = if kind == "wifi" { "Wi-Fi" } else { "Ethernet" };
    println!("Turning Wake-on-LAN {} for {iface} ({what}, {}).", if on { "on" } else { "off" }, mac_text(&mac));

    let wakeup = sys(&iface).join("device/power/wakeup");
    let text = if on { rule_text(kind, &mac, &tool) } else { String::new() };
    // SAFETY: geteuid has no preconditions.
    let mut cmd = if unsafe { libc::geteuid() } == 0 {
        Command::new("sh")
    } else {
        let mut sudo = Command::new("sudo");
        sudo.arg("sh");
        sudo
    };
    cmd.args(["-c", ROOT_SCRIPT, "sh"])
        .arg(&wakeup)
        .arg(if on { "enabled" } else { "disabled" })
        .arg(RULE)
        .arg(&text)
        .arg(&tool)
        .args(&now);
    let ok = cmd.status().map_err(|e| format!("can't run sudo: {e}"))?.success();
    if !ok {
        return Err("that didn't work (sudo needs a terminal to ask for your password)".into());
    }
    let _ = daemon::request(json!({"cmd": "wake-recheck"}));
    if !on {
        println!("Off. Phones stop offering to wake this computer the next time they connect.");
    } else if kind == "wifi" {
        println!("On. A phone on the same Wi-Fi can now wake this computer from sleep (not from power-off).");
        println!("Phones paired earlier learn it the next time they connect.");
    } else {
        println!("On. A phone on the same network can now wake this computer from sleep, and from");
        println!("power-off if Wake-on-LAN is also on in its firmware (BIOS/UEFI) settings.");
        println!("Phones paired earlier learn it the next time they connect.");
    }
    Ok(())
}

fn show() {
    let s = check();
    match (&s.interface, s.on) {
        (Some(i), true) => println!("Wake-on-LAN: on for {i} ({})", s.mac.as_ref().map(mac_text).unwrap_or_default()),
        (Some(i), false) if s.reason == "off" => println!("Wake-on-LAN: off for {i}. Turn it on with: omakeyd wake-on-lan on"),
        (Some(i), false) => println!("Wake-on-LAN: off for {i} ({})", s.reason),
        (None, _) => println!("Wake-on-LAN: off ({})", s.reason),
    }
}

/// `omakeyd wake-on-lan [on|off]`.
pub fn command(arg: Option<&str>) -> Result<(), String> {
    match arg {
        None | Some("status") => {
            show();
            Ok(())
        }
        Some("on") => set(true),
        Some("off") => set(false),
        Some(other) => Err(format!("unknown argument {other:?}; usage: omakeyd wake-on-lan [on|off]")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_parse_from_sysfs() {
        assert_eq!(parse_mac("e8:8d:a6:e0:89:93\n"), Some([0xe8, 0x8d, 0xa6, 0xe0, 0x89, 0x93]));
        assert_eq!(parse_mac("00:00:00:00:00:00"), None);
        assert_eq!(parse_mac("e8:8d:a6"), None);
        assert_eq!(mac_text(&[0xe8, 0x8d, 0xa6, 0xe0, 0x89, 0x93]), "e8:8d:a6:e0:89:93");
    }

    #[test]
    fn wowlan_show_output() {
        assert!(!wowlan_magic("WoWLAN is disabled.\n"));
        assert!(wowlan_magic("WoWLAN is enabled:\n * wake up on magic packet\n"));
        assert!(!wowlan_magic("WoWLAN is enabled:\n * wake up on disconnect\n"));
    }

    #[test]
    fn rules_arm_the_card_at_boot() {
        let mac = [0xe8, 0x8d, 0xa6, 0xe0, 0x89, 0x93];
        let wifi = rule_text("wifi", &mac, Path::new("/usr/bin/iw"));
        assert!(wifi.contains(r#"SUBSYSTEM=="ieee80211", KERNEL=="phy*", RUN+="/usr/bin/iw phy %k wowlan enable magic-packet""#));
        assert!(wifi.contains(r#"ATTR{address}=="e8:8d:a6:e0:89:93", TEST=="device/power/wakeup", ATTR{device/power/wakeup}="enabled""#));
        let wired = rule_text("ethernet", &mac, Path::new("/usr/bin/ethtool"));
        assert!(wired.contains(r#"ATTR{address}=="e8:8d:a6:e0:89:93", RUN+="/usr/bin/ethtool -s %k wol g""#));
    }

    #[test]
    fn wake_mac_only_while_on() {
        let mut s = WakeStatus { mac: Some([1; 6]), on: false, ..Default::default() };
        assert_eq!(s.wake_mac(), None);
        s.on = true;
        assert_eq!(s.wake_mac(), Some([1; 6]));
    }
}
