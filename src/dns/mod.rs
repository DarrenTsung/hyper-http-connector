use std::net::{
    Ipv4Addr, Ipv6Addr,
    SocketAddr,
    SocketAddrV4, SocketAddrV6,
};
use c_ares::AResults;

mod c_ares;

pub use self::c_ares::GLOBAL_RESOLVER;

pub struct IpAddrs {
    inner: SingleOrShifted<SocketAddr>,
}

impl IpAddrs {
    pub fn new(port: u16, a_results: AResults, index: usize) -> IpAddrs {
        // Sort list by ips returned, we need a set ordering for the round-robin
        // selection of ips to work correctly.
        let mut ips = a_results.iter().map(|res| res.ipv4()).collect::<Vec<_>>();
        ips.sort();

        let ips = ips.into_iter().map(|ip| {
            Some(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }).collect::<Vec<_>>();

        IpAddrs {
            inner: SingleOrShifted::ShiftedVec(ips, index),
        }
    }

    pub fn try_parse(host: &str, port: u16) -> Option<IpAddrs> {
        if let Ok(addr) = host.parse::<Ipv4Addr>() {
            let addr = SocketAddrV4::new(addr, port);
            return Some(IpAddrs {
                inner: SingleOrShifted::Single(Some(SocketAddr::V4(addr))),
            })
        }

        if let Ok(addr) = host.parse::<Ipv6Addr>() {
            let addr = SocketAddrV6::new(addr, port, 0, 0);
            return Some(IpAddrs {
                inner: SingleOrShifted::Single(Some(SocketAddr::V6(addr))),
            })
        }

        None
    }
}

impl Iterator for IpAddrs {
    type Item = SocketAddr;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

enum SingleOrShifted<T> {
    Single(Option<T>),
    ShiftedVec(Vec<Option<T>>, usize),
}

impl<T> Iterator for SingleOrShifted<T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        match self {
            SingleOrShifted::Single(ref mut item) => {
                item.take()
            },
            SingleOrShifted::ShiftedVec(ref mut items, ref mut index) => {
                let len = items.len();
                let ret = items[*index % len].take();
                *index += 1;
                ret
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_or_shifted_single_works() {
        let mut a = SingleOrShifted::Single(Some(10));
        assert_eq!(Some(10), a.next());
        assert_eq!(None, a.next());
    }

    #[test]
    fn single_or_shifted_shifted_works() {
        let mut a = SingleOrShifted::ShiftedVec(vec![Some(10), Some(20), Some(30)], 0);
        assert_eq!(Some(10), a.next());
        assert_eq!(Some(20), a.next());
        assert_eq!(Some(30), a.next());
        assert_eq!(None, a.next());

        let mut b = SingleOrShifted::ShiftedVec(vec![Some(10), Some(20), Some(30)], 2);
        assert_eq!(Some(30), b.next());
        assert_eq!(Some(10), b.next());
        assert_eq!(Some(20), b.next());
        assert_eq!(None, b.next());
    }
}
