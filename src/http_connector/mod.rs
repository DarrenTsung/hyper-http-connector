//! A duplicate of the default HttpConnector that comes with `hyper`.
//!
//! This is useful if you want to make modifications on it.
use c_ares::AResults;
use c_ares_resolver::CAresFuture;
use std::net::Ipv4Addr;

use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use antidote::{Mutex, RwLock};
use futures::future::{ExecuteError, Executor};
use futures::sync::oneshot;
use futures::{Future, Poll};
use futures_cpupool::Builder as CpuPoolBuilder;
use http::uri::Scheme;
use hyper::client::connect::{Connect, Destination};
use tokio_reactor::Handle;
use tokio_tcp::TcpStream;

use util::TimedCache;

mod http_connecting;

use self::http_connecting::{HttpConnecting, State};

pub(self) type ResultCache = TimedCache<Arc<String>, Vec<Ipv4Addr>>;
pub(self) struct RoundRobinMap(HashMap<Arc<String>, usize>);

impl RoundRobinMap {
    fn new() -> RoundRobinMap {
        RoundRobinMap(HashMap::new())
    }

    fn get_and_incr(&mut self, host: Arc<String>) -> usize {
        *self
            .0
            .entry(Arc::clone(&host))
            .and_modify(|e| *e = e.overflowing_add(1).0)
            .or_insert(0)
    }
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
    round_robin_map: Arc<Mutex<RoundRobinMap>>,
    result_cache: Arc<RwLock<ResultCache>>,
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
    where
        E: Executor<HttpConnectorBlockingTask> + Send + Sync,
    {
        HttpConnector {
            executor: HttpConnectExecutor(Arc::new(executor)),
            enforce_http: true,
            handle,
            keep_alive_timeout: None,
            nodelay: false,
            local_address: None,
            round_robin_map: Arc::new(Mutex::new(RoundRobinMap::new())),
            result_cache: Arc::new(RwLock::new(TimedCache::new())),
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
                return HttpConnecting::from_invalid_url(InvalidUrl::NotHttp, &self.handle);
            }
        } else if scheme.is_empty() {
            return HttpConnecting::from_invalid_url(InvalidUrl::MissingScheme, &self.handle);
        }

        if host.is_empty() {
            return HttpConnecting::from_invalid_url(InvalidUrl::MissingAuthority, &self.handle);
        }

        let port = match port {
            Some(port) => port,
            None => {
                if scheme == &Scheme::HTTPS {
                    443
                } else {
                    80
                }
            },
        };

        let host = Arc::new(host.into());

        HttpConnecting {
            state: State::Lazy(
                self.executor.clone(),
                host,
                port,
                self.local_address,
                Arc::clone(&self.round_robin_map),
                Arc::clone(&self.result_cache),
            ),
            handle: self.handle.clone(),
            keep_alive_timeout: self.keep_alive_timeout,
            nodelay: self.nodelay,
        }
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

#[derive(Clone)]
pub(self) struct HttpConnectExecutor(Arc<Executor<HttpConnectorBlockingTask> + Send + Sync>);

impl Executor<oneshot::Execute<CAresFuture<AResults>>> for HttpConnectExecutor {
    fn execute(
        &self,
        future: oneshot::Execute<CAresFuture<AResults>>,
    ) -> Result<(), ExecuteError<oneshot::Execute<CAresFuture<AResults>>>> {
        self.0
            .execute(HttpConnectorBlockingTask { work: future })
            .map_err(|err| ExecuteError::new(err.kind(), err.into_future().work))
    }
}

// Blocking task to be executed on a thread pool.
pub struct HttpConnectorBlockingTask {
    pub work: oneshot::Execute<CAresFuture<AResults>>,
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
