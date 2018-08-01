use c_ares_resolver::{Options, FutureResolver};

lazy_static! {
    pub static ref GLOBAL_RESOLVER: FutureResolver = {
        let mut options = Options::default();

        // enable round-robin rotating of the nameservers
        // returned, this way we can balance out load
        options.set_rotate();

        FutureResolver::with_options(options).unwrap()
    };
}
