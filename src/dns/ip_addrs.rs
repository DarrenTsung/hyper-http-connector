use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

pub(crate) struct IpAddrs {
    ips: Vec<SocketAddr>,
    current: usize,
    offset: usize,
}

impl IpAddrs {
    pub fn new(port: u16, mut ips: Vec<Ipv4Addr>, offset: usize) -> IpAddrs {
        ips.sort();

        let ips = ips
            .into_iter()
            .map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, port)))
            .collect::<Vec<_>>();

        IpAddrs {
            ips,
            current: 0,
            offset,
        }
    }

    pub fn try_parse(host: &str, port: u16) -> Option<IpAddrs> {
        if let Ok(addr) = host.parse::<Ipv4Addr>() {
            let addr = SocketAddrV4::new(addr, port);
            return Some(IpAddrs {
                ips: vec![SocketAddr::V4(addr)],
                current: 0,
                offset: 0,
            });
        }

        if let Ok(addr) = host.parse::<Ipv6Addr>() {
            let addr = SocketAddrV6::new(addr, port, 0, 0);
            return Some(IpAddrs {
                ips: vec![SocketAddr::V6(addr)],
                current: 0,
                offset: 0,
            });
        }

        None
    }
}

impl Iterator for IpAddrs {
    type Item = SocketAddr;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.ips.len() {
            return None;
        }

        let index = (self.current + self.offset) % self.ips.len();
        let item = self.ips[index];
        self.current += 1;
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_eq_ip {
        ($elem:expr, $expected:expr) => {
            match $elem {
                Some(SocketAddr::V4(addr)) => assert_eq!(addr.ip(), $expected),
                _ => panic!("Not a Some(SocketAddrV4)!"),
            }
        };
    }

    #[test]
    fn it_sorts_by_ip() {
        let mut a = IpAddrs::new(
            999,
            vec![
                Ipv4Addr::new(127, 0, 0, 1),
                Ipv4Addr::new(127, 0, 0, 4),
                Ipv4Addr::new(127, 0, 0, 3),
                Ipv4Addr::new(127, 0, 0, 2),
            ],
            0,
        );

        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 1));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 2));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 3));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 4));
        assert_eq!(a.next(), None);
    }

    #[test]
    fn shifted_offsets_work() {
        let mut a = IpAddrs::new(
            999,
            vec![
                Ipv4Addr::new(127, 0, 0, 1),
                Ipv4Addr::new(127, 0, 0, 4),
                Ipv4Addr::new(127, 0, 0, 3),
                Ipv4Addr::new(127, 0, 0, 2),
            ],
            2,
        );

        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 3));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 4));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 1));
        assert_eq_ip!(a.next(), &Ipv4Addr::new(127, 0, 0, 2));
        assert_eq!(a.next(), None);
    }
}
