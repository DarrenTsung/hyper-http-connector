use std;

use tokio;
use trust_dns_resolver::ResolverFuture;

lazy_static! {
    // First we need to setup the global Resolver
    pub static ref GLOBAL_DNS_RESOLVER: ResolverFuture = {
        use std::sync::{Arc, Mutex, Condvar};
        use std::thread;

        // We'll be using this condvar to get the Resolver from the thread...
        let pair = Arc::new((Mutex::new(None::<ResolverFuture>), Condvar::new()));
        let pair2 = pair.clone();


        // Spawn the runtime to a new thread...
        //
        // This thread will manage the actual resolution runtime
        thread::spawn(move || {
            println!("Starting up global resolver thread!");
            // A runtime for this new thread
            let mut runtime = tokio::runtime::current_thread::Runtime::new().expect("failed to launch Runtime");

            // our platform independent future, result, see next blocks
            let future;

            // To make this independent, if targeting macOS, BSD, Linux, or Windows, we can use the system's configuration:
            #[cfg(any(unix, windows))]
            {
                // use the system resolver configuration
                future = ResolverFuture::from_system_conf().expect("Failed to create ResolverFuture");
            }

            // For other operating systems, we can use one of the preconfigured definitions
            #[cfg(not(any(unix, windows)))]
            {
                // Directly reference the config types
                use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};

                // Get a new resolver with the google nameservers as the upstream recursive resolvers
                future = ResolverFuture::new(ResolverConfig::google(), ResolverOpts::default());
            }

            // this will block the thread until the Resolver is constructed with the above configuration
            let resolver = runtime.block_on(future).expect("Failed to create DNS resolver");

            let &(ref lock, ref cvar) = &*pair2;
            let mut started = lock.lock().unwrap();
            *started = Some(resolver);
            cvar.notify_one();
            drop(started);

            runtime.run().expect("Resolver Thread shutdown!");
            println!("Shutting down global resolver thread!");
        });

        // Wait for the thread to start up.
        let &(ref lock, ref cvar) = &*pair;
        let mut resolver = lock.lock().unwrap();
        while resolver.is_none() {
            resolver = cvar.wait(resolver).unwrap();
        }

        // take the started resolver
        let resolver = std::mem::replace(&mut *resolver, None);
        println!("Found a resolver: !");

        // set the global resolver
        resolver.expect("resolver should not be none")
    };
}
