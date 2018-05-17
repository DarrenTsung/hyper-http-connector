use std::net::{
    Ipv4Addr, Ipv6Addr,
    SocketAddr,
    SocketAddrV4, SocketAddrV6,
};
use std::vec;

use c_ares::AResults;

pub struct IpAddrs {
    inner: IpAddrsInner,
}

pub enum IpAddrsInner {
    AddrVec(vec::IntoIter<SocketAddr>),
    CAres(u16, AResults, usize),
}

impl IpAddrs {
    pub fn from_a_results(port: u16, a_results: AResults) -> IpAddrs {
        IpAddrs {
            inner: IpAddrsInner::CAres(port, a_results, 0),
        }
    }

    pub fn try_parse(host: &str, port: u16) -> Option<IpAddrs> {
        if let Ok(addr) = host.parse::<Ipv4Addr>() {
            let addr = SocketAddrV4::new(addr, port);
            return Some(IpAddrs { inner: IpAddrsInner::AddrVec(vec![SocketAddr::V4(addr)].into_iter()) })
        }
        if let Ok(addr) = host.parse::<Ipv6Addr>() {
            let addr = SocketAddrV6::new(addr, port, 0, 0);
            return Some(IpAddrs { inner: IpAddrsInner::AddrVec(vec![SocketAddr::V6(addr)].into_iter()) })
        }
        None
    }
}

impl Iterator for IpAddrs {
    type Item = SocketAddr;

    #[inline]
    fn next(&mut self) -> Option<SocketAddr> {
        match self.inner {
            IpAddrsInner::AddrVec(ref mut iter) => iter.next(),
            IpAddrsInner::CAres(ref port, ref mut results, ref mut index) => {
                let item = results.iter().nth(*index);
                let port = *port;
                *index += 1;
                item.map(|res| {
                    SocketAddr::V4(SocketAddrV4::new(res.ipv4(), port))
                })
            },
        }
    }
}
