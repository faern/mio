use std::io;
use std::mem::size_of;
use std::net::{self, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;
use std::convert::TryInto;
use std::os::windows::io::FromRawSocket;
use std::os::windows::raw::SOCKET as StdSocket; // winapi uses usize, stdlib uses u32/u64.

use winapi::ctypes::{c_char, c_int, c_ushort};
use winapi::shared::ws2def::{SOCKADDR_STORAGE, AF_INET, AF_INET6, SOCKADDR_IN};
use winapi::shared::ws2ipdef::SOCKADDR_IN6_LH;

use winapi::shared::minwindef::{BOOL, TRUE, FALSE};
use winapi::um::winsock2::{
    self, closesocket, linger, setsockopt, getsockopt, getsockname, PF_INET, PF_INET6, SOCKET, SOCKET_ERROR,
    SOCK_STREAM, SOL_SOCKET, SO_LINGER, SO_REUSEADDR, SO_RCVBUF, SO_SNDBUF,
};

use crate::sys::windows::net::{init, new_socket, socket_addr};

pub(crate) type TcpSocket = SOCKET;

pub(crate) fn new_v4_socket() -> io::Result<TcpSocket> {
    init();
    new_socket(PF_INET, SOCK_STREAM)
}

pub(crate) fn new_v6_socket() -> io::Result<TcpSocket> {
    init();
    new_socket(PF_INET6, SOCK_STREAM)
}

pub(crate) fn bind(socket: TcpSocket, addr: SocketAddr) -> io::Result<()> {
    use winsock2::bind;

    let (raw_addr, raw_addr_length) = socket_addr(&addr);
    syscall!(
        bind(socket, raw_addr.as_ptr(), raw_addr_length),
        PartialEq::eq,
        SOCKET_ERROR
    )?;
    Ok(())
}

pub(crate) fn connect(socket: TcpSocket, addr: SocketAddr) -> io::Result<net::TcpStream> {
    use winsock2::connect;

    let (raw_addr, raw_addr_length) = socket_addr(&addr);

    let res = syscall!(
        connect(socket, raw_addr.as_ptr(), raw_addr_length),
        PartialEq::eq,
        SOCKET_ERROR
    );

    match res {
        Err(err) if err.kind() != io::ErrorKind::WouldBlock => {
            Err(err)
        }
        _ => {
            Ok(unsafe { net::TcpStream::from_raw_socket(socket as StdSocket) })
        }
    }
}

pub(crate) fn listen(socket: TcpSocket, backlog: u32) -> io::Result<net::TcpListener> {
    use winsock2::listen;
    use std::convert::TryInto;

    let backlog = backlog.try_into().unwrap_or(i32::max_value());
    syscall!(listen(socket, backlog), PartialEq::eq, SOCKET_ERROR)?;
    Ok(unsafe { net::TcpListener::from_raw_socket(socket as StdSocket) })
}

pub(crate) fn close(socket: TcpSocket) {
    let _ = unsafe { closesocket(socket) };
}

pub(crate) fn set_reuseaddr(socket: TcpSocket, reuseaddr: bool) -> io::Result<()> {
    let val: BOOL = if reuseaddr { TRUE } else { FALSE };

    match unsafe { setsockopt(
        socket,
        SOL_SOCKET,
        SO_REUSEADDR,
        &val as *const _ as *const c_char,
        size_of::<BOOL>() as c_int,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

pub(crate) fn get_reuseaddr(socket: TcpSocket) -> io::Result<bool> {
    let mut optval: c_char = 0;
    let mut optlen = size_of::<BOOL>() as c_int;

    match unsafe { getsockopt(
        socket,
        SOL_SOCKET,
        SO_REUSEADDR,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(optval != 0),
    }
}

pub(crate) fn get_localaddr(socket: TcpSocket) -> io::Result<SocketAddr> {
    let mut storage: SOCKADDR_STORAGE = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&storage) as c_int;

    match unsafe { getsockname(
        socket,
        &mut storage as *mut _ as *mut _,
        &mut length
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => {
            if storage.ss_family as c_int == AF_INET {
                // Safety: if the ss_family field is AF_INET then storage must be a sockaddr_in.
                let addr: &SOCKADDR_IN = unsafe { &*(&storage as *const _ as *const SOCKADDR_IN) };
                let ip_bytes = unsafe { addr.sin_addr.S_un.S_un_b() };
                let ip = Ipv4Addr::from([ip_bytes.s_b1, ip_bytes.s_b2, ip_bytes.s_b3, ip_bytes.s_b4]);
                let port = u16::from_be(addr.sin_port);
                Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
            } else if storage.ss_family as c_int == AF_INET6 {
                // Safety: if the ss_family field is AF_INET6 then storage must be a sockaddr_in6.
                let addr: &SOCKADDR_IN6_LH = unsafe { &*(&storage as *const _ as *const SOCKADDR_IN6_LH) };
                let ip = Ipv6Addr::from(*unsafe { addr.sin6_addr.u.Byte() });
                let port = u16::from_be(addr.sin6_port);
                let scope_id = unsafe { *addr.u.sin6_scope_id() };
                Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, addr.sin6_flowinfo, scope_id)))
            } else {
                Err(std::io::ErrorKind::InvalidInput.into())
            }
        },
    }
}

pub(crate) fn set_linger(socket: TcpSocket, dur: Option<Duration>) -> io::Result<()> {
    let val: linger = linger {
        l_onoff: if dur.is_some() { 1 } else { 0 },
        l_linger: dur.map(|dur| dur.as_secs() as c_ushort).unwrap_or_default(),
    };

    match unsafe { setsockopt(
        socket,
        SOL_SOCKET,
        SO_LINGER,
        &val as *const _ as *const c_char,
        size_of::<linger>() as c_int,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

pub(crate) fn get_linger(socket: TcpSocket) -> io::Result<Option<Duration>> {
    let mut val: linger = unsafe { std::mem::zeroed() };
    let mut len = size_of::<linger>() as c_int;

    match unsafe { getsockopt(
        socket,
        SOL_SOCKET,
        SO_LINGER,
        &mut val as *mut _ as *mut _,
        &mut len,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => {
            if val.l_onoff == 0 {
                Ok(None)
            } else {
                Ok(Some(Duration::from_secs(val.l_linger as u64)))
            }
        },
    }
}


pub(crate) fn set_recv_buffer_size(socket: TcpSocket, size: u32) -> io::Result<()> {
    let size = size.try_into().ok().unwrap_or_else(i32::max_value);
    match unsafe { setsockopt(
        socket,
        SOL_SOCKET,
        SO_RCVBUF,
        &size as *const _ as *const c_char,
        size_of::<c_int>() as c_int
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

pub(crate) fn get_recv_buffer_size(socket: TcpSocket) -> io::Result<u32> {
    let mut optval: c_int = 0;
    let mut optlen = size_of::<c_int>() as c_int;
    match unsafe { getsockopt(
        socket,
        SOL_SOCKET,
        SO_RCVBUF,
        &mut optval as *mut _ as *mut _,
        &mut optlen as *mut _,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(optval as u32),
    }
}

pub(crate) fn set_send_buffer_size(socket: TcpSocket, size: u32) -> io::Result<()> {
    let size = size.try_into().ok().unwrap_or_else(i32::max_value);
    match unsafe { setsockopt(
        socket,
        SOL_SOCKET,
        SO_SNDBUF,
        &size as *const _ as *const c_char,
        size_of::<c_int>() as c_int
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

pub(crate) fn get_send_buffer_size(socket: TcpSocket) -> io::Result<u32> {
    let mut optval: c_int = 0;
    let mut optlen = size_of::<c_int>() as c_int;
    match unsafe { getsockopt(
        socket,
        SOL_SOCKET,
        SO_SNDBUF,
        &mut optval as *mut _ as *mut _,
        &mut optlen as *mut _,
    ) } {
        SOCKET_ERROR => Err(io::Error::last_os_error()),
        _ => Ok(optval as u32),
    }
}


pub(crate) fn accept(listener: &net::TcpListener) -> io::Result<(net::TcpStream, SocketAddr)> {
    // The non-blocking state of `listener` is inherited. See
    // https://docs.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-accept#remarks.
    listener.accept()
}
