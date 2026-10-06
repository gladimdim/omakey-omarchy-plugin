//! Files under ~/.config/omakey: host identity, paired devices, config.

use crate::protocol::{random_bytes, DeviceId, Key};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 47800;

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("omakey")
}

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("omakey")
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

pub fn unb64_key(s: &str) -> Option<Key> {
    URL_SAFE_NO_PAD.decode(s).ok()?.try_into().ok()
}

/// Write a private file atomically (mode 600, rename into place).
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(data)?;
    }
    fs::rename(tmp, path)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// Host name shown on the phone; defaults to the machine's hostname.
    #[serde(default)]
    pub name: Option<String>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for Config {
    fn default() -> Self {
        Config { port: DEFAULT_PORT, name: None }
    }
}

impl Config {
    pub fn load() -> Config {
        fs::read(config_dir().join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        write_private(&config_dir().join("config.json"), &serde_json::to_vec_pretty(self)?)
    }
}

#[derive(Serialize, Deserialize)]
struct HostFile {
    host_id: String,
}

/// The stable 8-byte host id, created on first run.
pub fn host_id() -> io::Result<DeviceId> {
    let path = config_dir().join("host.json");
    if let Ok(b) = fs::read(&path) {
        if let Ok(h) = serde_json::from_slice::<HostFile>(&b) {
            if let Some(id) = unhex::<8>(&h.host_id) {
                return Ok(id);
            }
        }
    }
    let id: DeviceId = random_bytes();
    write_private(&path, &serde_json::to_vec_pretty(&HostFile { host_id: hex(&id) })?)?;
    Ok(id)
}

pub fn host_name(config: &Config) -> String {
    config.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Omarchy".into())
    })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub key: String,
    pub paired_at: u64,
    #[serde(default)]
    pub last_seen: u64,
}

impl Device {
    pub fn key_bytes(&self) -> Option<Key> {
        unb64_key(&self.key)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Devices {
    pub devices: Vec<Device>,
}

impl Devices {
    fn path() -> PathBuf {
        config_dir().join("devices.json")
    }

    pub fn load() -> Devices {
        fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        write_private(&Self::path(), &serde_json::to_vec_pretty(self)?)
    }

    pub fn find(&self, id: &DeviceId) -> Option<&Device> {
        let hid = hex(id);
        self.devices.iter().find(|d| d.id == hid)
    }

    pub fn find_mut(&mut self, id: &DeviceId) -> Option<&mut Device> {
        let hid = hex(id);
        self.devices.iter_mut().find(|d| d.id == hid)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        self.devices.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let b = [0u8, 1, 0xab, 0xff, 9, 8, 7, 6];
        assert_eq!(unhex::<8>(&hex(&b)), Some(b));
        assert_eq!(unhex::<8>("zz"), None);
    }
}
