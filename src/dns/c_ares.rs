use c_ares_resolver::FutureResolver;

lazy_static! {
    pub static ref GLOBAL_RESOLVER: FutureResolver = { FutureResolver::new().unwrap() };
}
