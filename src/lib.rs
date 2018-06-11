//! A duplicate of the default HttpConnector that comes with `hyper`.
//!
//! This is useful if you want to make modifications on it.
#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate lazy_static;

extern crate http;
extern crate hyper;
extern crate net2;
extern crate tokio;
extern crate tokio_reactor;
extern crate tokio_tcp;
extern crate futures_cpupool;
extern crate c_ares_resolver;
extern crate c_ares;

use c_ares_resolver::CAresFuture;
use c_ares::AResults;

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use std::sync::Arc;

use futures::{Async, Future, Poll};
use futures::future::{Executor, ExecuteError};
use futures::sync::oneshot;
use http::uri::Scheme;
use hyper::client::connect::{Connect, Connected, Destination};
use net2::TcpBuilder;
use futures_cpupool::{Builder as CpuPoolBuilder};
use tokio_reactor::Handle;
use tokio_tcp::{ConnectFuture, TcpStream};

mod dns;

use self::dns::GLOBAL_RESOLVER;
use self::http_connector::HttpConnectorBlockingTask;

fn connect(
    addr: &SocketAddr,
    local_addr: &Option<IpAddr>,
    handle: &Option<Handle>,
) -> io::Result<ConnectFuture> {
    let builder = match addr {
        &SocketAddr::V4(_) => TcpBuilder::new_v4()?,
        &SocketAddr::V6(_) => TcpBuilder::new_v6()?,
    };

    if let Some(ref local_addr) = *local_addr {
        // Caller has requested this socket be bound before calling connect
        builder.bind(SocketAddr::new(local_addr.clone(), 0))?;
    } else if cfg!(windows) {
        // Windows requires a socket be bound before calling connect
        let any: SocketAddr = match addr {
            &SocketAddr::V4(_) => ([0, 0, 0, 0], 0).into(),
            &SocketAddr::V6(_) => ([0, 0, 0, 0, 0, 0, 0, 0], 0).into(),
        };
        builder.bind(any)?;
    }

    let handle = match *handle {
        Some(ref handle) => Cow::Borrowed(handle),
        None => Cow::Owned(Handle::current()),
    };

    Ok(TcpStream::connect_std(
        builder.to_tcp_stream()?,
        addr,
        &handle,
    ))
}

/// A connector for the `http` scheme.
///
/// Performs DNS resolution in a thread pool, and then connects over TCP.
#[derive(Clone)]
pub struct HttpConnector {
    executor: HttpConnectExecutor,
    enforce_http: bool,
    handle: Option<Handle>,
    keep_alive_timeout: Option<Duration>,
    nodelay: bool,
    local_address: Option<IpAddr>,
}

impl HttpConnector {
    /// Construct a new HttpConnector.
    ///
    /// Takes number of DNS worker threads.
    #[inline]
    pub fn new(threads: usize) -> HttpConnector {
        HttpConnector::new_with_handle_opt(threads, None)
    }

    /// Construct a new HttpConnector with a specific Tokio handle.
    pub fn new_with_handle(threads: usize, handle: Handle) -> HttpConnector {
        HttpConnector::new_with_handle_opt(threads, Some(handle))
    }

    fn new_with_handle_opt(threads: usize, handle: Option<Handle>) -> HttpConnector {
        let pool = CpuPoolBuilder::new()
        .name_prefix("hyper-dns")
        .pool_size(threads)
        .create();
        HttpConnector::new_with_executor(pool, handle)
    }

    /// Construct a new HttpConnector.
    ///
    /// Takes an executor to run blocking tasks on.
    pub fn new_with_executor<E: 'static>(executor: E, handle: Option<Handle>) -> HttpConnector
    where E: Executor<HttpConnectorBlockingTask> + Send + Sync
    {
        HttpConnector {
            executor: HttpConnectExecutor(Arc::new(executor)),
            enforce_http: true,
            handle,
            keep_alive_timeout: None,
            nodelay: false,
            local_address: None,
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

    /// Set that all sockets have `SO_NODELAY` set to the supplied value `nodelay`.
    ///
    /// Default is `false`.
    #[inline]
    pub fn set_nodelay(&mut self, nodelay: bool) {
        self.nodelay = nodelay;
    }

    /// Set that all sockets are bound to the configured address before connection.
    ///
    /// If `None`, the sockets will not be bound.
    ///
    /// Default is `None`.
    #[inline]
    pub fn set_local_address(&mut self, addr: Option<IpAddr>) {
        self.local_address = addr;
    }
}

impl fmt::Debug for HttpConnector {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("HttpConnector").finish()
    }
}

impl Connect for HttpConnector {
    type Transport = TcpStream;
    type Error = io::Error;
    type Future = HttpConnecting;

    fn connect(&self, dst: Destination) -> Self::Future {
        let scheme = dst.scheme();
        let host = dst.host();
        let port = dst.port();
        trace!(
            "Http::connect; scheme={}, host={}, port={:?}",
            scheme,
            host,
            port,
        );

        if self.enforce_http {
            if scheme != &Scheme::HTTP {
                return invalid_url(InvalidUrl::NotHttp, &self.handle);
            }
        } else if scheme.is_empty() {
            return invalid_url(InvalidUrl::MissingScheme, &self.handle);
        }

        if host.is_empty() {
            return invalid_url(InvalidUrl::MissingAuthority, &self.handle);
        }

        let port = match port {
            Some(port) => port,
            None => if scheme == &Scheme::HTTPS {
                443
            } else {
                80
            },
        };

        HttpConnecting {
            state: State::Lazy(self.executor.clone(), host.into(), port, self.local_address),
            handle: self.handle.clone(),
            keep_alive_timeout: self.keep_alive_timeout,
            nodelay: self.nodelay,
        }
    }
}

#[inline]
fn invalid_url(err: InvalidUrl, handle: &Option<Handle>) -> HttpConnecting {
    HttpConnecting {
        state: State::Error(Some(io::Error::new(io::ErrorKind::InvalidInput, err))),
        handle: handle.clone(),
        keep_alive_timeout: None,
        nodelay: false,
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
    handle: Option<Handle>,
    keep_alive_timeout: Option<Duration>,
    nodelay: bool,
}

enum State {
    Lazy(HttpConnectExecutor, String, u16, Option<IpAddr>),
    Resolving(oneshot::SpawnHandle<AResults, c_ares::Error>, u16, Option<IpAddr>),
    Connecting(ConnectingTcp),
    Error(Option<io::Error>),
}

impl Future for HttpConnecting {
    type Item = (TcpStream, Connected);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            let state;
            match self.state {
                State::Lazy(ref executor, ref mut host, port, local_addr) => {
                    // If the host is already an IP addr (v4 or v6),
                    // skip resolving the dns and start connecting right away.
                    if let Some(addrs) = dns::IpAddrs::try_parse(host, port) {
                        state = State::Connecting(ConnectingTcp {
                            addrs,
                            local_addr,
                            current: None
                        })
                    } else {
                        let work = GLOBAL_RESOLVER.query_a(&host);
                        state = State::Resolving(oneshot::spawn(work, executor), port, local_addr);
                    }
                },
                State::Resolving(ref mut future, port, local_addr) => {
                    match future.poll()
                        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
                    {
                        Async::NotReady => return Ok(Async::NotReady),
                        Async::Ready(a_results) => {
                            state = State::Connecting(ConnectingTcp {
                                addrs: dns::IpAddrs::new(port, a_results),
                                local_addr,
                                current: None,
                            })
                        }
                    };
                }
                State::Connecting(ref mut c) => {
                    let sock = try_ready!(c.poll(&self.handle));

                    if let Some(dur) = self.keep_alive_timeout {
                        sock.set_keepalive(Some(dur))?;
                    }

                    sock.set_nodelay(self.nodelay)?;

                    return Ok(Async::Ready((sock, Connected::new())));
                }
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
    local_addr: Option<IpAddr>,
    current: Option<ConnectFuture>,
}

impl ConnectingTcp {
    // not a Future, since passing a &Handle to poll
    fn poll(&mut self, handle: &Option<Handle>) -> Poll<TcpStream, io::Error> {
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
                            *current = connect(&addr, &self.local_addr, handle)?;
                            continue;
                        }
                    }
                }
            } else if let Some(addr) = self.addrs.next() {
                debug!("connecting to {}", addr);
                self.current = Some(connect(&addr, &self.local_addr, handle)?);
                continue;
            }

            return Err(err.take().expect("missing connect error"));
        }
    }
}

// Make this Future unnameable outside of this crate.
mod http_connector {
    use super::*;
    // Blocking task to be executed on a thread pool.
    pub struct HttpConnectorBlockingTask {
        pub(super) work: oneshot::Execute<CAresFuture<AResults>>
    }

    impl fmt::Debug for HttpConnectorBlockingTask {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.pad("HttpConnectorBlockingTask")
        }
    }

    impl Future for HttpConnectorBlockingTask {
        type Item = ();
        type Error = ();

        fn poll(&mut self) -> Poll<(), ()> {
            self.work.poll()
        }
    }
}

#[derive(Clone)]
struct HttpConnectExecutor(Arc<Executor<HttpConnectorBlockingTask> + Send + Sync>);

impl Executor<oneshot::Execute<CAresFuture<AResults>>> for HttpConnectExecutor {
    fn execute(&self, future: oneshot::Execute<CAresFuture<AResults>>) -> Result<(), ExecuteError<oneshot::Execute<CAresFuture<AResults>>>> {
        self.0.execute(HttpConnectorBlockingTask { work: future })
        .map_err(|err| ExecuteError::new(err.kind(), err.into_future().work))
    }
}

// Can't use these tests because they use non-public
// variables to construct Destination
/*
#[cfg(test)]
mod tests {
    use std::io;
    use futures::Future;
    use super::{Connect, Destination, HttpConnector};

    #[test]
    fn test_errors_missing_authority() {
        let uri = "/foo/bar?baz".parse().unwrap();
        let dst = Destination {
            uri,
        };
        let connector = HttpConnector::new(1);

        assert_eq!(connector.connect(dst).wait().unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_errors_enforce_http() {
        let uri = "https://example.domain/foo/bar?baz".parse().unwrap();
        let dst = Destination {
            uri,
        };
        let connector = HttpConnector::new(1);

        assert_eq!(connector.connect(dst).wait().unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }


    #[test]
    fn test_errors_missing_scheme() {
        let uri = "example.domain".parse().unwrap();
        let dst = Destination {
            uri,
        };
        let connector = HttpConnector::new(1);

        assert_eq!(connector.connect(dst).wait().unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
*/
