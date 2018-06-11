use std::net::{
    Ipv4Addr, Ipv6Addr,
    SocketAddr,
    SocketAddrV4, SocketAddrV6,
};
use std::vec;
use c_ares::AResults;

mod c_ares;

pub use self::c_ares::GLOBAL_RESOLVER;

pub struct IpAddrs {
    iter: vec::IntoIter<SocketAddr>,
}

impl IpAddrs {
    pub fn new(port: u16, a_results: AResults) -> IpAddrs {
        let ips = a_results.iter().map(|res| {
            SocketAddr::V4(SocketAddrV4::new(res.ipv4(), port))
        }).collect::<Vec<_>>();
        IpAddrs {
            iter: ips.into_iter(),
        }
    }

    pub fn try_parse(host: &str, port: u16) -> Option<IpAddrs> {
        if let Ok(addr) = host.parse::<Ipv4Addr>() {
            let addr = SocketAddrV4::new(addr, port);
            return Some(IpAddrs { iter: vec![SocketAddr::V4(addr)].into_iter() })
        }
        if let Ok(addr) = host.parse::<Ipv6Addr>() {
            let addr = SocketAddrV6::new(addr, port, 0, 0);
            return Some(IpAddrs { iter: vec![SocketAddr::V6(addr)].into_iter() })
        }
        None
    }
}

impl Iterator for IpAddrs {
    type Item = SocketAddr;
    #[inline]
    fn next(&mut self) -> Option<SocketAddr> {
        self.iter.next()
    }
}
