//! A duplicate of the default HttpConnector that comes with `hyper`.
//!
//! This is useful if you want to make modifications on it.
#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate lazy_static;

extern crate antidote;
extern crate c_ares;
extern crate c_ares_resolver;
extern crate futures_cpupool;
extern crate http;
extern crate hyper;
extern crate net2;
extern crate tokio;
extern crate tokio_reactor;
extern crate tokio_tcp;

mod dns;
mod http_connector;
mod util;

pub use self::http_connector::{HttpConnector, HttpConnectorBlockingTask};
