use c_ares::{self, AResults};

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use antidote::{Mutex, RwLock};
use futures::sync::oneshot;
use futures::{Async, Future, Poll};
use hyper::client::connect::Connected;
use net2::TcpBuilder;
use tokio_reactor::Handle;
use tokio_tcp::{ConnectFuture, TcpStream};

use dns::{IpAddrs, GLOBAL_RESOLVER};
use http_connector::{HttpConnectExecutor, InvalidUrl, ResultCache, RoundRobinMap};

/// A Future representing work to connect to a URL.
#[must_use = "futures do nothing unless polled"]
pub struct HttpConnecting {
    pub(super) state: State,
    pub(super) handle: Option<Handle>,
    pub(super) keep_alive_timeout: Option<Duration>,
    pub(super) nodelay: bool,
}

impl HttpConnecting {
    #[inline]
    pub(super) fn from_invalid_url(err: InvalidUrl, handle: &Option<Handle>) -> HttpConnecting {
        HttpConnecting {
            state: State::Error(Some(io::Error::new(io::ErrorKind::InvalidInput, err))),
            handle: handle.clone(),
            keep_alive_timeout: None,
            nodelay: false,
        }
    }
}

pub(super) enum State {
    Lazy(
        HttpConnectExecutor,
        Arc<String>,
        u16,
        Option<IpAddr>,
        Arc<Mutex<RoundRobinMap>>,
        Arc<RwLock<ResultCache>>,
    ),
    Resolving(
        oneshot::SpawnHandle<AResults, c_ares::Error>,
        Arc<String>,
        u16,
        Option<IpAddr>,
        Arc<Mutex<RoundRobinMap>>,
        Arc<RwLock<ResultCache>>,
    ),
    Connecting(ConnectingTcp),
    Error(Option<io::Error>),
}

impl Future for HttpConnecting {
    type Item = (TcpStream, Connected);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            let state = match self.state {
                State::Lazy(
                    ref executor,
                    ref host,
                    port,
                    local_addr,
                    ref round_robin_map,
                    ref result_cache,
                ) => {
                    // If the host is already an IP addr (v4 or v6),
                    // skip resolving the dns and start connecting right away.
                    if let Some(addrs) = IpAddrs::try_parse(host, port) {
                        State::Connecting(ConnectingTcp {
                            addrs,
                            local_addr,
                            current: None,
                        })
                    } else {
                        if let Some(ip_addrs) = result_cache.read().get(host) {
                            trace!("ResultCache - got cached ips!");
                            let shift_index =
                                round_robin_map.lock().get_and_incr(Arc::clone(&host));
                            State::Connecting(ConnectingTcp {
                                addrs: IpAddrs::new(port, ip_addrs.clone(), shift_index),
                                local_addr,
                                current: None,
                            })
                        } else {
                            trace!("ResultCache - no cached ips!");
                            let work = GLOBAL_RESOLVER.query_a(host);
                            State::Resolving(
                                oneshot::spawn(work, executor),
                                Arc::clone(host),
                                port,
                                local_addr,
                                Arc::clone(round_robin_map),
                                Arc::clone(result_cache),
                            )
                        }
                    }
                },
                State::Resolving(
                    ref mut future,
                    ref host,
                    port,
                    local_addr,
                    ref round_robin_map,
                    ref result_cache,
                ) => match future
                    .poll()
                    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
                {
                    Async::NotReady => return Ok(Async::NotReady),
                    Async::Ready(a_results) => {
                        let min_ttl = a_results.iter().map(|res| res.ttl()).min();
                        let ips = a_results
                            .into_iter()
                            .map(|res| res.ipv4())
                            .collect::<Vec<_>>();
                        let shift_index = round_robin_map.lock().get_and_incr(Arc::clone(&host));

                        trace!("ResultCache - putting in the cache for {}!", host);
                        // min_ttl will be None if no ip records were found
                        if let Some(min_ttl) = min_ttl {
                            result_cache.write().set(
                                Arc::clone(&host),
                                ips.clone(),
                                Duration::from_secs(min_ttl as u64),
                            );
                        }
                        State::Connecting(ConnectingTcp {
                            addrs: IpAddrs::new(port, ips, shift_index),
                            local_addr,
                            current: None,
                        })
                    },
                },
                State::Connecting(ref mut c) => {
                    let sock = try_ready!(c.poll(&self.handle));

                    if let Some(dur) = self.keep_alive_timeout {
                        sock.set_keepalive(Some(dur))?;
                    }

                    sock.set_nodelay(self.nodelay)?;

                    return Ok(Async::Ready((sock, Connected::new())));
                },
                State::Error(ref mut e) => return Err(e.take().expect("polled more than once")),
            };
            self.state = state;
        }
    }
}

impl fmt::Debug for HttpConnecting {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("HttpConnecting")
    }
}

pub(super) struct ConnectingTcp {
    addrs: IpAddrs,
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
                    },
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
