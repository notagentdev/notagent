//! A minimal MCP server that misbehaves on demand, for the MCP client tests.
//!
//! Written by hand against the wire format rather than built on the SDK,
//! because most of what the tests need is a server behaving badly: hanging,
//! dying mid-call, answering with an invalid schema, closing the transport.
//! An SDK server is built to do none of those.
//!
//! The behaviour is chosen by the first argument. With none it is a
//! well-behaved server offering one `echo` tool.
//!
//! ```text
//! mcp_test_server [behaviour]
//!
//!   normal            answer everything correctly
//!   hang-handshake    accept the connection, never answer initialize
//!   hang-call*        answer initialize and tools/list, never answer a call
//!   die-on-call       exit the moment a call arrives
//!   close-after-call  answer one call, then exit
//!   error-on-call     answer a call with a JSON-RPC error
//!   invalid-schema    offer one good tool and one whose schema is not an object
//!   huge-result       answer a call with a megabyte of text
//!   audio-result      answer a call with an audio content block
//!   noisy-stderr      write to stderr without ever reading stdin
//!   needs-auth        fail the handshake the way an unauthenticated server does
//!   many-tools        offer more tools than the client accepts
//!   die-once          exit on the first call ever made, answer every later one
//!
//! A second argument names a journal file. Every `tools/call` appends a line to
//! it, which is how the tests count attempts: a triage that retries where it
//! should not shows up as two lines where there should be one. `die-once` uses
//! the same file to know whether it has died yet, so it survives the reconnect
//! that follows.
//! ```

use std::io::{BufRead, BufReader, Read, Write};

fn main() {
    let behaviour = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "normal".to_owned());

    if behaviour == "noisy-stderr" {
        // Never reads stdin: the client's stderr drain is what keeps this from
        // blocking once the pipe fills.
        let mut stderr = std::io::stderr();
        loop {
            let _ = writeln!(stderr, "{}", "x".repeat(4096));
            let _ = stderr.flush();
        }
    }

    let journal = std::env::args().nth(2);
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        // A notification carries no id and expects no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();

        match method.as_str() {
            "initialize" => {
                if behaviour == "hang-handshake" {
                    park();
                }
                if behaviour == "needs-auth" {
                    send_error(&id, -32001, "Unauthorized: HTTP 401");
                    continue;
                }
                send_result(&id, initialize_result());
            }
            "tools/list" => send_result(&id, tools_result(&behaviour)),
            "tools/call" => {
                let attempts = record_attempt(journal.as_deref());
                match behaviour.as_str() {
                    // A suffix makes a distinct process name without a distinct
                    // behaviour, so two tests that count running servers by
                    // their argument do not count each other's.
                    name if name.starts_with("hang-call") => park(),
                    "die-on-call" => std::process::exit(1),
                    // The journal survives the process, so the replacement
                    // started by the client's reconnect knows to answer.
                    "die-once" if attempts <= 1 => std::process::exit(1),
                    "error-on-call" => {
                        send_error(&id, -32602, "the server rejected these arguments");
                        continue;
                    }
                    _ => {}
                }
                send_result(&id, call_result(&behaviour, &message));
                if behaviour == "close-after-call" {
                    // Answer once, then go: the client's next request meets a
                    // closed transport rather than a slow one.
                    return;
                }
            }
            "ping" => send_result(&id, serde_json::json!({})),
            _ => send_error(&id, -32601, &format!("unknown method `{method}`")),
        }
    }
}

/// Appends one line to the journal and returns how many calls it now holds.
///
/// Without a journal there is nothing to count and nothing to remember, which
/// is the ordinary case.
fn record_attempt(journal: Option<&str>) -> usize {
    let Some(path) = journal else {
        return 0;
    };
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let count = existing.lines().filter(|line| !line.is_empty()).count() + 1;
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => return count,
    };
    let _ = writeln!(file, "call");
    count
}

/// Blocks forever without spinning, so the client's deadline is what ends it.
fn park() -> ! {
    let mut sink = Vec::new();
    // Reading stdin to end never completes while the client holds the pipe.
    let _ = std::io::stdin().read_to_end(&mut sink);
    loop {
        std::thread::park();
    }
}

fn initialize_result() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "mcp-test-server", "version": "0.0.0" }
    })
}

fn echo_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "echo",
        "description": "Returns the text it was given.",
        "inputSchema": {
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }
    })
}

fn tools_result(behaviour: &str) -> serde_json::Value {
    let tools = match behaviour {
        "invalid-schema" => serde_json::json!([
            echo_tool(),
            { "name": "broken", "description": "Its schema is a string.", "inputSchema": "nope" }
        ]),
        "many-tools" => serde_json::Value::Array(
            (0..500)
                .map(|index| {
                    let mut tool = echo_tool();
                    tool["name"] = serde_json::Value::String(format!("echo_{index}"));
                    tool
                })
                .collect(),
        ),
        _ => serde_json::json!([echo_tool()]),
    };
    serde_json::json!({ "tools": tools })
}

fn call_result(behaviour: &str, message: &serde_json::Value) -> serde_json::Value {
    match behaviour {
        "huge-result" => serde_json::json!({
            "content": [{ "type": "text", "text": "y".repeat(1024 * 1024) }],
            "isError": false
        }),
        "audio-result" => serde_json::json!({
            "content": [{ "type": "audio", "data": "AAAA", "mimeType": "audio/wav" }],
            "isError": false
        }),
        _ => {
            let text = message
                .pointer("/params/arguments/text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            serde_json::json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            })
        }
    }
}

fn send_result(id: &serde_json::Value, result: serde_json::Value) {
    send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn send_error(id: &serde_json::Value, code: i32, message: &str) {
    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }));
}

fn send(message: serde_json::Value) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}
