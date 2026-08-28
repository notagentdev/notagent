use notagent::cli::args::Args;
use notagent::cli::initial_message::{InitialMessageInput, build_initial_message};

fn create_args(messages: &[&str]) -> Args {
    Args {
        messages: messages
            .iter()
            .map(|message| (*message).to_owned())
            .collect(),
        ..Args::default()
    }
}

#[test]
fn merges_piped_stdin_with_the_first_cli_message_into_one_prompt() {
    let mut parsed = create_args(&["Summarize the text given"]);

    let result = build_initial_message(
        &mut parsed,
        InitialMessageInput {
            stdin_content: Some("README contents\n".to_owned()),
            ..InitialMessageInput::default()
        },
    );

    assert_eq!(
        result.initial_message.as_deref(),
        Some("README contents\nSummarize the text given")
    );
    assert!(parsed.messages.is_empty());
}

#[test]
fn uses_stdin_as_the_initial_prompt_when_no_cli_message_is_present() {
    let mut parsed = create_args(&[]);

    let result = build_initial_message(
        &mut parsed,
        InitialMessageInput {
            stdin_content: Some("README contents".to_owned()),
            ..InitialMessageInput::default()
        },
    );

    assert_eq!(result.initial_message.as_deref(), Some("README contents"));
    assert!(parsed.messages.is_empty());
}

#[test]
fn combines_stdin_file_text_and_first_cli_message_in_one_prompt() {
    let mut parsed = create_args(&["Explain it", "Second message"]);

    let result = build_initial_message(
        &mut parsed,
        InitialMessageInput {
            stdin_content: Some("stdin\n".to_owned()),
            file_text: Some("file\n".to_owned()),
            ..InitialMessageInput::default()
        },
    );

    assert_eq!(
        result.initial_message.as_deref(),
        Some("stdin\nfile\nExplain it")
    );
    assert_eq!(parsed.messages, vec!["Second message".to_owned()]);
}
