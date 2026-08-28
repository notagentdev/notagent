fn main() {
    // ring and aws-lc-rs are both linked; without an explicit default rustls
    // panics on the first TLS handshake (see notagent_ai's helper doc).
    notagent_ai::install_default_crypto_provider();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit_code = runtime.block_on(notagent::cli::run(args));
    std::process::exit(exit_code);
}
