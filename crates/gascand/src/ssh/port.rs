use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

pub(crate) const AUTOMATIC_PORT_ATTEMPTS: usize = 8;

#[derive(Debug)]
pub struct PortReservation {
    port: u16,
    listener: TcpListener,
}

impl PortReservation {
    pub fn reserve() -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let port = match listener.local_addr()? {
            std::net::SocketAddr::V4(address)
                if *address.ip() == Ipv4Addr::LOCALHOST && address.port() >= 1024 =>
            {
                address.port()
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "automatic SSH reservation was not an unprivileged IPv4 loopback port",
                ));
            }
        };
        Ok(Self { port, listener })
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn release(self) {
        drop(self.listener);
    }
}
