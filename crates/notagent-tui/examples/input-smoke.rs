//! Manual smoke test for the input path on a real terminal emulator.
//! Verification criterion of plan task 1–4: Kitty keyboard negotiation,
//! bracketed paste and mouse events have to work on at least two real
//! emulators, once through the Kitty protocol path and once through the legacy
//! path. Run it in each emulator and press the listed keys:
//! ```text
//! cargo run -p notagent-tui --example input-smoke
//! ```
//! It prints the raw bytes, the parsed key and the negotiated protocol state,
//! and exits on Ctrl+C.

use std::cell::Cell;
use std::rc::Rc;

use notagent_tui::keys::{is_kitty_protocol_active, parse_key};
use notagent_tui::terminal::{ProcessTerminal, PumpResult, Terminal, TerminalPump};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (mut terminal, mut pump) = ProcessTerminal::new().into_shared();
    println!("input smoke test — press keys, paste text, click and scroll; Ctrl+C exits");

    let done = Rc::new(Cell::new(false));
    let stop = done.clone();
    terminal.start(
        Box::new(move |data: &str| {
            if data == "\x03" {
                stop.set(true);
            }
            let key = parse_key(data);
            println!(
                "bytes {data:?} | key {key:?} | kitty {}\r",
                is_kitty_protocol_active()
            );
        }),
        Box::new(|| println!("resize\r")),
    );

    // Mouse reporting and bracketed paste, as the alternate screen enables them.
    terminal.write("\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h");

    while pump.pump().await != PumpResult::Eof && !done.get() {}

    terminal.write("\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l");
    terminal.stop();
}
