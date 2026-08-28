use notagent_ai::api::bedrock_converse_stream::{
    bedrock_failure_diagnostic_details, format_bedrock_error,
};
use serde_json::{Value, json};

const VALIDATION_MESSAGE: &str = "The provided model identifier is invalid.";
const REQUEST_ID: &str = "11111111-2222-3333-4444-555555555555";

fn details(
    status: Option<u16>,
    exception_name: Option<&str>,
    request_id: Option<&str>,
    fallback_request_id: Option<&str>,
) -> Value {
    Value::Object(
        bedrock_failure_diagnostic_details(status, exception_name, request_id, fallback_request_id)
            .unwrap_or_default(),
    )
}

#[test]
fn records_status_error_code_and_request_id_for_a_non_2xx_from_client_send() {
    let details = bedrock_failure_diagnostic_details(
        Some(400),
        Some("ValidationException"),
        Some(REQUEST_ID),
        None,
    )
    .expect("details");

    assert_eq!(
        Value::Object(details.clone()),
        json!({ "status": 400, "errorCode": "ValidationException", "requestId": REQUEST_ID })
    );
    // The diagnostic carries no `error`; only `details`, `timestamp` and `type` exist.
    let mut keys: Vec<&String> = details.keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["errorCode", "requestId", "status"]);
}

#[test]
fn leaves_error_message_untouched_so_retry_classification_is_unaffected() {
    assert_eq!(
        format_bedrock_error(
            Some("ValidationException"),
            Some(400),
            None,
            VALIDATION_MESSAGE
        ),
        format!("Validation error: {VALIDATION_MESSAGE}")
    );
}

#[test]
fn reports_only_the_request_id_for_a_modeled_mid_stream_exception() {
    // The SDK throws a bare object literal here, so the code is genuinely unavailable.
    assert_eq!(
        details(None, None, None, Some(REQUEST_ID)),
        json!({ "requestId": REQUEST_ID })
    );
}

#[test]
fn captures_the_error_code_for_an_unmodeled_mid_stream_error() {
    assert_eq!(
        details(
            None,
            Some("ModelStreamErrorException"),
            None,
            Some(REQUEST_ID)
        ),
        json!({ "errorCode": "ModelStreamErrorException", "requestId": REQUEST_ID })
    );
}

#[test]
fn does_not_report_a_transport_failure_name_as_a_provider_error_code() {
    // Real errors with informative names, but not AWS codes; modeled ones end in "Exception".
    assert_eq!(
        details(None, Some("TimeoutError"), None, Some(REQUEST_ID)),
        json!({ "requestId": REQUEST_ID })
    );
}

#[test]
fn emits_no_diagnostic_when_the_failure_carries_no_provider_metadata() {
    assert_eq!(
        bedrock_failure_diagnostic_details(None, None, None, None),
        None
    );
    assert_eq!(
        format_bedrock_error(None, None, None, "socket hang up"),
        "socket hang up"
    );
}

#[test]
fn drops_header_derived_values_that_exceed_the_length_bound() {
    let long_name = format!("{}Exception", "E".repeat(5000));
    let long_request_id = "R".repeat(5000);
    assert_eq!(
        details(Some(400), Some(&long_name), Some(&long_request_id), None),
        json!({ "status": 400 })
    );
}

#[test]
fn omits_the_sdk_unknown_placeholder_instead_of_reporting_it_as_a_code() {
    // The SDK's fallback when the response carried no `x-amzn-errortype`.
    assert_eq!(
        details(Some(403), Some("Unknown"), Some(REQUEST_ID), None),
        json!({ "status": 403, "requestId": REQUEST_ID })
    );
}
