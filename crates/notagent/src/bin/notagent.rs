/// The stack of the thread that drives the CLI. Session trees are about as deep
/// as a session is long, and the RPC protocol sends them nested: serde_json
/// clones, prints and drops such a value by recursion, which overflows the
/// default 8 MiB after a few thousand entries. The size is only reserved; pages
/// are committed as they are touched.
const CLI_STACK_BYTES: usize = 256 * 1024 * 1024;

fn main() {
    // ring and aws-lc-rs are both linked; without an explicit default rustls
    // panics on the first TLS handshake (see notagent_ai's helper doc).
    notagent_ai::install_default_crypto_provider();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = std::sync::Arc::new(runtime);
    let driver_runtime = std::sync::Arc::clone(&runtime);
    let driver_args = args.clone();
    let spawned = std::thread::Builder::new()
        .name("notagent-cli".to_owned())
        .stack_size(CLI_STACK_BYTES)
        .spawn(move || driver_runtime.block_on(notagent::cli::run(driver_args)));
    let exit_code = match spawned {
        // A panic was already reported by the hook; exit as an uncaught one would.
        Ok(driver) => driver.join().unwrap_or(101),
        // No thread could be created: run where we are, with the default stack.
        Err(_) => runtime.block_on(notagent::cli::run(args)),
    };
    std::process::exit(exit_code);
}
