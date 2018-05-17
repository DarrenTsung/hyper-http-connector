//! A duplicate of the default HttpConnector that comes with `hyper`.
//!
//! This is useful if you want to make modifications on it.
#[macro_use] extern crate log;
#[macro_use] extern crate futures;

extern crate hyper;
extern crate tokio_core;
extern crate tokio_service;
extern crate trust_dns_resolver;

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::mem;
use std::time::Duration;

use futures::{Future, Poll, Async};
use hyper::Uri;
use tokio_core::net::{TcpStream, TcpStreamNew};
use tokio_core::reactor::Handle;
use tokio_service::Service;

use trust_dns_resolver::config::*;
use trust_dns_resolver::lookup_ip::{LookupIpFuture};
use trust_dns_resolver::ResolverFuture;

mod dns;

/// A connector for the `http` scheme.
pub struct HttpConnector {
    enforce_http: bool,
    handle: Handle,
    keep_alive_timeout: Option<Duration>,
    resolver: ResolverFuture,
}

impl HttpConnector {
    /// Construct a new HttpConnector.
    #[inline]
    pub fn new(handle: &Handle) -> HttpConnector {
        HttpConnector {
            enforce_http: true,
            handle: handle.clone(),
            keep_alive_timeout: None,
            resolver: ResolverFuture::new(
                ResolverConfig::default(),
                ResolverOpts::default(),
                handle,
            ),
        }
    }

    /// Option to enforce all `Uri`s have the `http` scheme.
    ///
    /// Enabled by default.
    #[inline]
    pub fn enforce_http(&mut self, is_enforced: bool) {
        self.enforce_http = is_enforced;
    }

    /// Set that all sockets have `SO_KEEPALIVE` set with the supplied duration.
    ///
    /// If `None`, the option will not be set.
    ///
    /// Default is `None`.
    #[inline]
    pub fn set_keepalive(&mut self, dur: Option<Duration>) {
        self.keep_alive_timeout = dur;
    }
}

impl fmt::Debug for HttpConnector {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("HttpConnector")
            .finish()
    }
}

impl Service for HttpConnector {
    type Request = Uri;
    type Response = TcpStream;
    type Error = io::Error;
    type Future = HttpConnecting;

    fn call(&self, uri: Uri) -> Self::Future {
        trace!("Http::connect({:?})", uri);

        if self.enforce_http {
            if uri.scheme() != Some("http") {
                return invalid_url(InvalidUrl::NotHttp, &self.handle);
            }
        } else if uri.scheme().is_none() {
            return invalid_url(InvalidUrl::MissingScheme, &self.handle);
        }

        let host = match uri.host() {
            Some(s) => s,
            None => return invalid_url(InvalidUrl::MissingAuthority, &self.handle),
        };
        let port = match uri.port() {
            Some(port) => port,
            None => match uri.scheme() {
                Some("https") => 443,
                _ => 80,
            },
        };

        let state = if let Some(addrs) = dns::IpAddrs::try_parse(host, port) {
            State::Connecting(ConnectingTcp {
                addrs: addrs,
                current: None
            })
        } else {
            let work = self.resolver.lookup_ip(&host);
            State::Resolving(work, port)
        };

        HttpConnecting {
            state,
            handle: self.handle.clone(),
            keep_alive_timeout: self.keep_alive_timeout,
        }
    }
}

#[inline]
fn invalid_url(err: InvalidUrl, handle: &Handle) -> HttpConnecting {
    HttpConnecting {
        state: State::Error(Some(io::Error::new(io::ErrorKind::InvalidInput, err))),
        handle: handle.clone(),
        keep_alive_timeout: None,
    }
}

#[derive(Debug, Clone, Copy)]
enum InvalidUrl {
    MissingScheme,
    NotHttp,
    MissingAuthority,
}

impl fmt::Display for InvalidUrl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.description())
    }
}

impl StdError for InvalidUrl {
    fn description(&self) -> &str {
        match *self {
            InvalidUrl::MissingScheme => "invalid URL, missing scheme",
            InvalidUrl::NotHttp => "invalid URL, scheme must be http",
            InvalidUrl::MissingAuthority => "invalid URL, missing domain",
        }
    }
}

/// A Future representing work to connect to a URL.
#[must_use = "futures do nothing unless polled"]
pub struct HttpConnecting {
    state: State,
    handle: Handle,
    keep_alive_timeout: Option<Duration>,
}

enum State {
    // Lazy(&mut ResolverFuture, String, u16),
    Resolving(LookupIpFuture, u16),
    Connecting(ConnectingTcp),
    Error(Option<io::Error>),
}

impl Future for HttpConnecting {
    type Item = TcpStream;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            let state;
            match self.state {
                // State::Lazy(ref mut resolver, ref mut host, port) => {
                //     // If the host is already an IP addr (v4 or v6),
                //     // skip resolving the dns and start connecting right away.
                //     if let Some(addrs) = dns::IpAddrs::try_parse(host, port) {
                //         state = State::Connecting(ConnectingTcp {
                //             addrs: addrs,
                //             current: None
                //         })
                //     } else {
                //         let host = mem::replace(host, String::new());
                //
                //         let work = resolver.lookup_ip(&host);
                //         state = State::Resolving(work, port);
                //     }
                // },
                State::Resolving(ref mut future, ref port) => {
                    match try!(future.poll()) {
                        Async::NotReady => return Ok(Async::NotReady),
                        Async::Ready(lookup_ip) => {
                            state = State::Connecting(ConnectingTcp {
                                addrs: dns::IpAddrs::new(*port, lookup_ip),
                                current: None,
                            })
                        }
                    };
                },
                State::Connecting(ref mut c) => {
                    let sock = try_ready!(c.poll(&self.handle));

                    if let Some(dur) = self.keep_alive_timeout {
                        sock.set_keepalive(Some(dur))?;
                    }

                    return Ok(Async::Ready(sock));
                },
                State::Error(ref mut e) => return Err(e.take().expect("polled more than once")),
            }
            self.state = state;
        }
    }
}

impl fmt::Debug for HttpConnecting {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("HttpConnecting")
    }
}

struct ConnectingTcp {
    addrs: dns::IpAddrs,
    current: Option<TcpStreamNew>,
}

impl ConnectingTcp {
    // not a Future, since passing a &Handle to poll
    fn poll(&mut self, handle: &Handle) -> Poll<TcpStream, io::Error> {
        let mut err = None;
        loop {
            if let Some(ref mut current) = self.current {
                match current.poll() {
                    Ok(ok) => return Ok(ok),
                    Err(e) => {
                        trace!("connect error {:?}", e);
                        err = Some(e);
                        if let Some(addr) = self.addrs.next() {
                            debug!("connecting to {}", addr);
                            *current = TcpStream::connect(&addr, handle);
                            continue;
                        }
                    }
                }
            } else if let Some(addr) = self.addrs.next() {
                debug!("connecting to {}", addr);
                self.current = Some(TcpStream::connect(&addr, handle));
                continue;
            }

            return Err(err.take().expect("missing connect error"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HttpConnector;

    use hyper::client::Connect;
    use std::io;
    use tokio_core::reactor::Core;

    #[test]
    fn test_errors_missing_authority() {
        let mut core = Core::new().unwrap();
        let url = "/foo/bar?baz".parse().unwrap();
        let connector = HttpConnector::new(&core.handle());

        assert_eq!(core.run(connector.connect(url)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_errors_enforce_http() {
        let mut core = Core::new().unwrap();
        let url = "https://example.domain/foo/bar?baz".parse().unwrap();
        let connector = HttpConnector::new(&core.handle());

        assert_eq!(core.run(connector.connect(url)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }


    #[test]
    fn test_errors_missing_scheme() {
        let mut core = Core::new().unwrap();
        let url = "example.domain".parse().unwrap();
        let connector = HttpConnector::new(&core.handle());

        assert_eq!(core.run(connector.connect(url)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
