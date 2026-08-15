//! Binary root of the `notagent` command.
//!
//! Rust reserves a file for the binary crate root; `src/main.rs` is the module
//! of `main.ts` instead (see `crate::main_app`). Everything here is the tokio
//! entry point plus the process setup of `packages/coding-agent/src/cli.ts`,
//! which lives in `notagent::cli::run`.

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit_code = runtime.block_on(notagent::cli::run(args));
    std::process::exit(exit_code);
}
