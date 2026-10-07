//! The UDP socket. Replies leave from the address the phone sent to
//! (IP_PKTINFO), so a desktop with several addresses answers from the
//! right one.

use std::io;
use std::mem::{size_of, size_of_val, zeroed};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::Duration;

pub struct UdpServer {
    sock: UdpSocket,
}

/// The local address a datagram arrived on.
pub type Local = Option<Ipv4Addr>;

fn setsockopt_int(sock: &UdpSocket, level: i32, name: i32, value: i32) -> io::Result<()> {
    // SAFETY: passes a pointer to a live int and its size.
    let ok = unsafe {
        libc::setsockopt(sock.as_raw_fd(), level, name, (&value as *const i32).cast(), size_of::<i32>() as libc::socklen_t)
    };
    if ok != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn sockaddr(a: SocketAddrV4) -> libc::sockaddr_in {
    // SAFETY: plain C struct, all zeroes is valid.
    let mut sa: libc::sockaddr_in = unsafe { zeroed() };
    sa.sin_family = libc::AF_INET as libc::sa_family_t;
    sa.sin_port = a.port().to_be();
    sa.sin_addr = libc::in_addr { s_addr: u32::from(*a.ip()).to_be() };
    sa
}

impl UdpServer {
    pub fn bind(port: u16) -> Result<UdpServer, String> {
        let sock = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| format!("can't bind UDP port {port}: {e}"))?;
        sock.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        if let Err(e) = setsockopt_int(&sock, libc::IPPROTO_IP, libc::IP_PKTINFO, 1) {
            eprintln!("omakeyd: can't set IP_PKTINFO: {e}");
        }
        // DSCP EF on our ACKs, as the phone does on its packets: Wi-Fi WMM
        // sends them in the voice queue, the lowest-latency one.
        if let Err(e) = setsockopt_int(&sock, libc::IPPROTO_IP, libc::IP_TOS, 0xB8) {
            eprintln!("omakeyd: can't set IP_TOS: {e}");
        }
        Ok(UdpServer { sock })
    }

    /// One datagram: its length, the sender, and the address it was sent to.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr, Local)> {
        // SAFETY: plain C structs, all zeroes is valid.
        let mut name: libc::sockaddr_in = unsafe { zeroed() };
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        let mut control = [0u64; 8];
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_name = (&mut name as *mut libc::sockaddr_in).cast();
        msg.msg_namelen = size_of::<libc::sockaddr_in>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = size_of_val(&control) as _;
        // SAFETY: every pointer in msg refers to a live local buffer.
        let n = unsafe { libc::recvmsg(self.sock.as_raw_fd(), &mut msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut local = None;
        // SAFETY: walks the control messages the kernel just wrote.
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::IPPROTO_IP && (*c).cmsg_type == libc::IP_PKTINFO {
                    let info: libc::in_pktinfo = std::ptr::read_unaligned(libc::CMSG_DATA(c).cast());
                    local = Some(Ipv4Addr::from(u32::from_be(info.ipi_spec_dst.s_addr)));
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        let from = SocketAddrV4::new(Ipv4Addr::from(u32::from_be(name.sin_addr.s_addr)), u16::from_be(name.sin_port));
        Ok((n as usize, SocketAddr::V4(from), local))
    }

    /// Send `data` to `to` from the local address `local`, when known.
    pub fn send(&self, data: &[u8], to: SocketAddr, local: Local) -> io::Result<()> {
        let (SocketAddr::V4(to), Some(local)) = (to, local) else {
            return self.sock.send_to(data, to).map(|_| ());
        };
        let mut name = sockaddr(to);
        let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
        let mut control = [0u64; 4];
        // SAFETY: plain C struct, all zeroes is valid.
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_name = (&mut name as *mut libc::sockaddr_in).cast();
        msg.msg_namelen = size_of::<libc::sockaddr_in>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        // SAFETY: CMSG_SPACE/LEN only compute sizes; the header and data
        // written below fit in `control` (32 bytes >= CMSG_SPACE(12)).
        let n = unsafe {
            msg.msg_controllen = libc::CMSG_SPACE(size_of::<libc::in_pktinfo>() as u32) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::IPPROTO_IP;
            (*c).cmsg_type = libc::IP_PKTINFO;
            (*c).cmsg_len = libc::CMSG_LEN(size_of::<libc::in_pktinfo>() as u32) as _;
            let info = libc::in_pktinfo {
                ipi_ifindex: 0,
                ipi_spec_dst: libc::in_addr { s_addr: u32::from(local).to_be() },
                ipi_addr: libc::in_addr { s_addr: 0 },
            };
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast(), info);
            libc::sendmsg(self.sock.as_raw_fd(), &msg, 0)
        };
        if n < 0 {
            // E.g. the address went away since the request came in.
            return self.sock.send_to(data, to).map(|_| ());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_come_from_the_address_the_request_went_to() {
        let server = UdpServer::bind(0).unwrap();
        let port = server.sock.local_addr().unwrap().port();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // 127.0.0.2 is also loopback; a reply from 127.0.0.1 would be wrong.
        client.send_to(b"ping", ("127.0.0.2", port)).unwrap();
        let mut buf = [0u8; 16];
        let (n, from, local) = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert_eq!(local, Some(Ipv4Addr::new(127, 0, 0, 2)));
        server.send(b"pong", from, local).unwrap();
        let (n, reply_from) = client.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"pong");
        assert_eq!(reply_from, SocketAddr::from(([127, 0, 0, 2], port)));
    }
}
