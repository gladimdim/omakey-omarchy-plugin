mod bluetooth;
mod client;
mod clipboard;
mod daemon;
mod hypr;
mod keyboard;
mod net;
mod notify;
mod protocol;
mod qr;
mod server;
mod store;
mod theme;
mod vectors;

#[cfg(test)]
mod tests;

use client::{Client, PairInfo, Reply};
use serde_json::json;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "omakeyd — use your phone as a real keyboard on Omarchy

Usage:
  omakeyd run [--port N] [--dry-run]   run the service (systemd starts this)
  omakeyd pair [--no-wait]             show a QR code to pair a phone
  omakeyd cancel-pair                  close the pairing window
  omakeyd status [--json]              connection status
  omakeyd devices                      list paired phones
  omakeyd forget <device-id>           unpair a phone
  omakeyd rename <device-id> <name>    rename a phone
  omakeyd config [--name NAME] [--port N]
                                       show or change settings (restart to apply)
  omakeyd test-client <pair-uri> [--addr IP] [--text TEXT]
                                       pair as a fake phone and type TEXT
                                       (types into the focused window!)
  omakeyd test-vectors                 print protocol test vectors (JSON)

--dry-run prints key events instead of typing them.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();

    let result = match cmd {
        "run" => {
            let port = match opt("--port").map(|p| p.parse::<u16>()) {
                Some(Ok(p)) => Some(p),
                Some(Err(_)) => return fail("--port needs a number"),
                None => None,
            };
            daemon::run(port, flag("--dry-run"))
        }
        "pair" => pair(!flag("--no-wait")),
        "cancel-pair" => daemon::request(json!({"cmd": "cancel-pair"})).map(|_| ()),
        "status" => status(flag("--json")),
        "devices" => devices(),
        "forget" => match args.get(1) {
            Some(id) => daemon::request(json!({"cmd": "forget", "id": id})).map(|_| println!("Forgot {id}.")),
            None => Err("usage: omakeyd forget <device-id>".into()),
        },
        "rename" => match (args.get(1), args.get(2)) {
            (Some(id), Some(_)) => {
                let name = args[2..].join(" ");
                daemon::request(json!({"cmd": "rename", "id": id, "name": name})).map(|_| ())
            }
            _ => Err("usage: omakeyd rename <device-id> <name>".into()),
        },
        "test-client" => match args.get(1) {
            Some(uri) => test_client(uri, opt("--addr"), opt("--text").unwrap_or_default()),
            None => Err("usage: omakeyd test-client <pair-uri> [--addr IP] [--text TEXT]".into()),
        },
        "config" => config(opt("--name"), opt("--port")),
        "test-vectors" => {
            println!("{}", serde_json::to_string_pretty(&vectors::generate()).unwrap());
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("omakeyd: {msg}");
    ExitCode::FAILURE
}

fn pair(wait: bool) -> Result<(), String> {
    let v = daemon::request(json!({"cmd": "pair"}))?;
    let uri = v["uri"].as_str().unwrap_or_default().to_string();
    let start_count = v["paired_count"].as_u64().unwrap_or(0);
    let left = v["expires_at"].as_u64().unwrap_or(0).saturating_sub(store::unix_now());
    println!("{}", qr::terminal(&uri));
    println!("Fingerprint: {} — the phone shows the same code", v["fingerprint"].as_str().unwrap_or("?"));
    println!("Scan with the Omakey app (or paste this link into it):\n{uri}\n");
    if v["reused"] == json!(true) {
        println!("(This pairing code was already open; it is still the one to use.)");
    }
    if !wait {
        return Ok(());
    }
    println!("Waiting for your phone… (Ctrl+C to stop; the code expires in {}:{:02})", left / 60, left % 60);
    let deadline = Instant::now() + Duration::from_secs(left);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        let s = daemon::request(json!({"cmd": "status"}))?;
        if s["paired"]["count"].as_u64().unwrap_or(0) > start_count {
            println!("Paired with {}. You can start typing.", s["paired"]["name"].as_str().unwrap_or("your phone"));
            return Ok(());
        }
        if s["pairing"].is_null() {
            return Err("pairing was cancelled".into());
        }
    }
    Err("the pairing code expired; run omakeyd pair again".into())
}

fn config(name: Option<String>, port: Option<String>) -> Result<(), String> {
    let mut c = store::Config::load();
    if let Some(n) = name {
        let n: String = n.trim().chars().take(64).collect();
        c.name = if n.is_empty() { None } else { Some(n) };
    }
    if let Some(p) = port {
        c.port = p.parse::<u16>().ok().filter(|p| *p >= 1024).ok_or("--port needs a number from 1024 to 65535")?;
    }
    c.save().map_err(|e| format!("can't save config: {e}"))?;
    let shown = serde_json::json!({ "name": store::host_name(&c), "custom_name": c.name, "port": c.port });
    println!("{shown}");
    Ok(())
}

fn status(as_json: bool) -> Result<(), String> {
    let s = daemon::request(json!({"cmd": "status"}))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&s).unwrap());
        return Ok(());
    }
    println!("omakeyd on UDP {} as \"{}\"", s["port"], s["host_name"].as_str().unwrap_or(""));
    let addrs: Vec<&str> = s["addresses"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    println!("Addresses: {}", if addrs.is_empty() { "none (no network?)".into() } else { addrs.join(", ") });
    println!("Keyboard: {}", s["uinput"].as_str().unwrap_or("?"));
    let bt = &s["bluetooth"];
    match bt["state"].as_str() {
        Some("on") => println!("Bluetooth: on ({})", bt["address"].as_str().unwrap_or("")),
        Some(state) => println!("Bluetooth: {state} ({})", bt["reason"].as_str().unwrap_or("")),
        None => {}
    }
    let n = s["connected"].as_u64().unwrap_or(0);
    println!("Connected phones: {n}");
    if !s["pairing"].is_null() {
        println!("Pairing window open.");
    }
    Ok(())
}

fn devices() -> Result<(), String> {
    let s = daemon::request(json!({"cmd": "status"}))?;
    let list = s["devices"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("No paired phones. Pair one with: omakeyd pair");
    }
    for d in list {
        println!(
            "{}  {}{}",
            d["id"].as_str().unwrap_or(""),
            d["name"].as_str().unwrap_or(""),
            if d["connected"] == json!(true) { "  (connected)" } else { "" }
        );
    }
    Ok(())
}

fn char_to_keys(c: char) -> Option<(bool, u16)> {
    const ROW_Q: &str = "qwertyuiop";
    const ROW_A: &str = "asdfghjkl";
    const ROW_Z: &str = "zxcvbnm";
    let lower = c.to_ascii_lowercase();
    let shift = c.is_ascii_uppercase();
    let code = if let Some(i) = ROW_Q.find(lower) {
        16 + i as u16
    } else if let Some(i) = ROW_A.find(lower) {
        30 + i as u16
    } else if let Some(i) = ROW_Z.find(lower) {
        44 + i as u16
    } else if c == '0' {
        11
    } else if c.is_ascii_digit() {
        2 + (c as u16 - '1' as u16)
    } else if c == ' ' {
        57
    } else if c == '\n' {
        28
    } else {
        return None;
    };
    Some((shift, code))
}

fn test_client(uri: &str, addr: Option<String>, text: String) -> Result<(), String> {
    let info = PairInfo::parse(uri).ok_or("that isn't an omakey://pair link")?;
    let host = addr.or_else(|| info.addrs.first().cloned()).ok_or("the link has no address; pass --addr")?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.connect((host.as_str(), info.port)).map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
    let mut client = Client::new(info.device_id, info.key);
    let t0 = Instant::now();
    let ms = || t0.elapsed().as_millis() as u32;
    let mut buf = [0u8; 1500];

    for _ in 0..20 {
        sock.send(&client.hello("omakeyd test-client")).map_err(|e| e.to_string())?;
        if let Ok(n) = sock.recv(&mut buf) {
            match client.on_packet(&buf[..n]) {
                Some(Reply::Welcome { host_name }) => {
                    println!("Connected to {host_name}");
                    break;
                }
                Some(Reply::Reject(_)) => return Err("the desktop doesn't know this device (link already used?)".into()),
                _ => {}
            }
        }
    }
    if !client.connected() {
        return Err(format!("no answer from {host}:{}", info.port));
    }

    let mut rtts = Vec::new();
    let mut pump = |client: &mut Client, pkt: Option<Vec<u8>>, rtts: &mut Vec<u32>| {
        if let Some(p) = pkt {
            let _ = sock.send(&p);
        }
        // Wait for the ACK, resending like the phone does.
        for _ in 0..10 {
            if let Ok(n) = sock.recv(&mut buf) {
                if let Some(Reply::Ack { client_time_ms, .. }) = client.on_packet(&buf[..n]) {
                    rtts.push(ms().wrapping_sub(client_time_ms));
                    if !client.has_unacked() {
                        return;
                    }
                }
            } else if let Some(p) = client.input(ms()) {
                let _ = sock.send(&p);
            }
        }
    };

    for c in text.chars() {
        let Some((shift, code)) = char_to_keys(c) else { continue };
        if shift {
            let p = client.key(42, true, ms());
            pump(&mut client, p, &mut rtts);
        }
        let p = client.key(code, true, ms());
        pump(&mut client, p, &mut rtts);
        let p = client.key(code, false, ms());
        pump(&mut client, p, &mut rtts);
        if shift {
            let p = client.key(42, false, ms());
            pump(&mut client, p, &mut rtts);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    for _ in 0..5 {
        let p = client.input(ms());
        pump(&mut client, p, &mut rtts);
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(bye) = client.bye() {
        let _ = sock.send(&bye);
    }
    if !rtts.is_empty() {
        rtts.sort();
        println!(
            "{} acks, ping min {} ms, median {} ms, max {} ms",
            rtts.len(),
            rtts[0],
            rtts[rtts.len() / 2],
            rtts[rtts.len() - 1]
        );
    }
    Ok(())
}
