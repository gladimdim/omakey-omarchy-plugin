//! Whose turn it is at the seat. The virtual keyboard and mouse join seat0,
//! and the session in front there reads them, whoever it belongs to. So
//! omakeyd types only while seat0's active session is its own user's: when
//! another user switches in, a paired phone must not type into their
//! session. Removing our uaccess ACL on /dev/uinput doesn't stop us, as the
//! descriptor stays open; this does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use zbus::blocking::{proxy, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const LOGIN1: &str = "org.freedesktop.login1";
const SEAT0: &str = "/org/freedesktop/login1/seat/seat0";

/// Start watching logind. The flag is true while seat0's active session is
/// ours. It is set before this returns, and is false whenever logind can't
/// tell us: no answer means no typing.
pub fn watch() -> Arc<AtomicBool> {
    let in_front = Arc::new(AtomicBool::new(false));
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    let first = Connection::system().map_err(|e| e.to_string()).and_then(|conn| {
        let seat = seat_proxy(&conn)?;
        in_front.store(active_is_ours(&conn, &seat, uid), Ordering::SeqCst);
        Ok((conn, seat))
    });
    let flag = in_front.clone();
    std::thread::Builder::new()
        .name("seat".into())
        .spawn(move || follow(first, &flag, uid))
        .unwrap();
    in_front
}

/// Follow seat0's ActiveSession until the bus goes, then reconnect.
fn follow(first: Result<(Connection, Proxy<'static>), String>, in_front: &AtomicBool, uid: u32) {
    let mut link = first;
    loop {
        match &link {
            Ok((conn, seat)) => {
                // Subscribe before reading, so no switch falls in between.
                let changes = seat.receive_property_changed::<(String, OwnedObjectPath)>("ActiveSession");
                in_front.store(active_is_ours(conn, seat, uid), Ordering::SeqCst);
                for change in changes {
                    let mine = change.get().is_ok_and(|(_, path)| owned_by(conn, &path, uid));
                    in_front.store(mine, Ordering::SeqCst);
                }
                eprintln!("omakeyd: lost logind; not typing until it answers");
            }
            Err(e) => eprintln!("omakeyd: can't reach logind ({e}); not typing until it answers"),
        }
        in_front.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(2));
        link = Connection::system().map_err(|e| e.to_string()).and_then(|conn| {
            let seat = seat_proxy(&conn)?;
            Ok((conn, seat))
        });
    }
}

fn seat_proxy(conn: &Connection) -> Result<Proxy<'static>, String> {
    proxy::Builder::new(conn)
        .destination(LOGIN1)
        .and_then(|b| b.path(SEAT0))
        .and_then(|b| b.interface("org.freedesktop.login1.Seat"))
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())
}

/// Whether seat0's active session belongs to `uid`. Any doubt is a no.
fn active_is_ours(conn: &Connection, seat: &Proxy, uid: u32) -> bool {
    seat.get_property::<(String, OwnedObjectPath)>("ActiveSession")
        .is_ok_and(|(_, path)| owned_by(conn, &path, uid))
}

/// Whether the session at `path` belongs to `uid`; "/" is no session.
fn owned_by(conn: &Connection, path: &OwnedObjectPath, uid: u32) -> bool {
    if path.as_str() == "/" {
        return false;
    }
    let session = proxy::Builder::<Proxy>::new(conn)
        .destination(LOGIN1)
        .and_then(|b| b.path(path.as_ref()))
        .and_then(|b| b.interface("org.freedesktop.login1.Session"))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(|b| b.build());
    let owner = session.and_then(|s| s.get_property::<(u32, OwnedObjectPath)>("User"));
    matches!(owner, Ok((owner, _)) if owner == uid)
}
