//! Ports of `packages/ai/test/oauth-device-code.test.ts` and the flow-level cases of
//! `oauth-auth.test.ts` that do not need a live endpoint (PKCE, token parsing, JWT claim
//! extraction, Copilot base-URL derivation, poll classification).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notagent_ai::auth::oauth::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, poll_device_code_flow,
};
use notagent_ai::auth::oauth::{
    anthropic, github_copilot, kimi_coding, openai_codex, pkce, radius, xai,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

fn body(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object")
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

#[test]
fn pkce_produces_a_base64url_verifier_and_its_sha256_challenge() {
    let pkce = pkce::generate_pkce();
    // 32 random bytes, base64url without padding.
    assert_eq!(pkce.verifier.len(), 43);
    assert!(
        !pkce.verifier.contains('+')
            && !pkce.verifier.contains('/')
            && !pkce.verifier.contains('=')
    );
    assert_eq!(pkce.challenge, pkce::challenge_for(&pkce.verifier));
    assert_ne!(pkce.verifier, pkce.challenge);
    // Two calls differ.
    assert_ne!(pkce::generate_pkce().verifier, pkce.verifier);
}

#[test]
fn the_pkce_challenge_matches_the_rfc_7636_test_vector() {
    // RFC 7636 appendix B.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(
        pkce::challenge_for(verifier),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

// ---------------------------------------------------------------------------
// device-code polling
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn polls_immediately_and_returns_the_completed_value() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);
    let value = poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: Some(2.0),
            expires_in_seconds: Some(30.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if call == 0 {
                    DeviceCodePollResult::Pending
                } else {
                    DeviceCodePollResult::Complete { value: "token" }
                }
            }
        },
    )
    .await
    .expect("completes");
    assert_eq!(value, "token");
    assert_eq!(polls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn can_wait_before_the_first_poll() {
    let started = tokio::time::Instant::now();
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);
    let value = poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: Some(2.0),
            expires_in_seconds: Some(30.0),
            wait_before_first_poll: true,
            signal: CancellationToken::new(),
        },
        || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { DeviceCodePollResult::Complete { value: "token" } }
        },
    )
    .await
    .expect("completes");
    assert_eq!(value, "token");
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the first poll waits one interval"
    );
}

#[tokio::test(start_paused = true)]
async fn slow_down_increases_the_interval_by_five_seconds() {
    let intervals = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let last = Arc::new(Mutex::new(tokio::time::Instant::now()));
    let recorder = Arc::clone(&intervals);
    let last_seen = Arc::clone(&last);
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);

    poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: Some(2.0),
            expires_in_seconds: Some(120.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            let recorder = Arc::clone(&recorder);
            let last_seen = Arc::clone(&last_seen);
            async move {
                let now = tokio::time::Instant::now();
                let mut last = last_seen.lock().expect("poisoned");
                recorder
                    .lock()
                    .expect("poisoned")
                    .push(now.duration_since(*last));
                *last = now;
                match call {
                    0 => DeviceCodePollResult::SlowDown {
                        interval_seconds: None,
                    },
                    _ => DeviceCodePollResult::Complete { value: "token" },
                }
            }
        },
    )
    .await
    .expect("completes");

    let intervals = intervals.lock().expect("poisoned").clone();
    // 2 s base plus the RFC's 5 s increment.
    assert!(intervals[1] >= Duration::from_secs(7), "{:?}", intervals[1]);
}

#[tokio::test(start_paused = true)]
async fn a_server_interval_wins_over_the_rfc_increment() {
    let last = Arc::new(Mutex::new(tokio::time::Instant::now()));
    let gaps = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);
    let recorder = Arc::clone(&gaps);
    let last_seen = Arc::clone(&last);

    poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: Some(2.0),
            expires_in_seconds: Some(120.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            let recorder = Arc::clone(&recorder);
            let last_seen = Arc::clone(&last_seen);
            async move {
                let now = tokio::time::Instant::now();
                let mut last = last_seen.lock().expect("poisoned");
                recorder
                    .lock()
                    .expect("poisoned")
                    .push(now.duration_since(*last));
                *last = now;
                match call {
                    0 => DeviceCodePollResult::SlowDown {
                        interval_seconds: Some(11.0),
                    },
                    _ => DeviceCodePollResult::Complete { value: "token" },
                }
            }
        },
    )
    .await
    .expect("completes");

    let gaps = gaps.lock().expect("poisoned").clone();
    assert!(gaps[1] >= Duration::from_secs(11), "{:?}", gaps[1]);
    assert!(gaps[1] < Duration::from_secs(12), "{:?}", gaps[1]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_poll_surfaces_its_message() {
    let error = poll_device_code_flow::<(), _, _>(
        DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(30.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || async {
            DeviceCodePollResult::Failed {
                message: "denied".to_string(),
            }
        },
    )
    .await
    .expect_err("must fail");
    assert_eq!(error.to_string(), "denied");
}

#[tokio::test(start_paused = true)]
async fn the_timeout_message_mentions_slow_down_when_one_occurred() {
    let plain = poll_device_code_flow::<(), _, _>(
        DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(2.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || async { DeviceCodePollResult::Pending },
    )
    .await
    .expect_err("times out");
    assert_eq!(plain.to_string(), "Device flow timed out");

    let after_slow_down = poll_device_code_flow::<(), _, _>(
        DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(3.0),
            wait_before_first_poll: false,
            signal: CancellationToken::new(),
        },
        || async {
            DeviceCodePollResult::SlowDown {
                interval_seconds: None,
            }
        },
    )
    .await
    .expect_err("times out");
    assert!(
        after_slow_down.to_string().contains("clock drift"),
        "{after_slow_down}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_aborted_signal_cancels_the_poll_loop() {
    let signal = CancellationToken::new();
    signal.cancel();
    let error = poll_device_code_flow::<(), _, _>(
        DeviceCodePollOptions {
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(30.0),
            wait_before_first_poll: false,
            signal,
        },
        || async { DeviceCodePollResult::Pending },
    )
    .await
    .expect_err("cancelled");
    assert_eq!(error.to_string(), "Login cancelled");
}

// ---------------------------------------------------------------------------
// Flow adapters
// ---------------------------------------------------------------------------

#[test]
fn only_subscription_flows_report_themselves_as_subscriptions() {
    assert!(anthropic::anthropic_oauth().is_subscription());
    assert!(openai_codex::openai_codex_oauth().is_subscription());
    assert!(github_copilot::github_copilot_oauth().is_subscription());
    assert!(kimi_coding::kimi_coding_oauth().is_subscription());
    assert!(xai::xai_oauth().is_subscription());
    assert!(!notagent_ai::auth::oauth::openrouter::open_router_oauth().is_subscription());
}

#[tokio::test]
async fn access_tokens_become_api_keys() {
    let credential = notagent_ai::auth::types::OAuthCredential {
        access: "token".to_string(),
        refresh: "r".to_string(),
        expires: 0,
        extra: Default::default(),
    };
    for flow in [
        anthropic::anthropic_oauth(),
        openai_codex::openai_codex_oauth(),
        notagent_ai::auth::oauth::openrouter::open_router_oauth(),
        xai::xai_oauth(),
    ] {
        let auth = flow.to_auth(credential.clone()).await.expect("auth");
        assert_eq!(auth.api_key.as_deref(), Some("token"), "{}", flow.name());
    }
}

#[tokio::test]
async fn kimi_authenticates_through_the_authorization_header() {
    let credential = notagent_ai::auth::types::OAuthCredential {
        access: "token".to_string(),
        refresh: "r".to_string(),
        expires: 0,
        extra: Default::default(),
    };
    let auth = kimi_coding::kimi_coding_oauth()
        .to_auth(credential)
        .await
        .expect("auth");
    assert_eq!(auth.api_key, None);
    assert_eq!(
        auth.headers.expect("headers").get("Authorization"),
        Some(&Some("Bearer token".to_string()))
    );
}

#[tokio::test]
async fn openrouter_keeps_its_permanent_credential_on_refresh() {
    let credential = notagent_ai::auth::types::OAuthCredential {
        access: "key".to_string(),
        refresh: String::new(),
        expires: 9_007_199_254_740_991,
        extra: Default::default(),
    };
    let refreshed = notagent_ai::auth::oauth::openrouter::open_router_oauth()
        .refresh(credential.clone(), CancellationToken::new())
        .await
        .expect("refresh");
    assert_eq!(refreshed, credential);
}

#[tokio::test]
async fn copilot_derives_the_base_url_from_the_token_proxy_endpoint() {
    let access = "tid=abc;exp=123;proxy-ep=proxy.enterprise.example;rest";
    let credential = notagent_ai::auth::types::OAuthCredential {
        access: access.to_string(),
        refresh: "r".to_string(),
        expires: 0,
        extra: Default::default(),
    };
    let auth = github_copilot::github_copilot_oauth()
        .to_auth(credential)
        .await
        .expect("auth");
    assert_eq!(auth.api_key.as_deref(), Some(access));
    assert_eq!(
        auth.base_url.as_deref(),
        Some("https://api.enterprise.example")
    );
}

#[tokio::test]
async fn copilot_falls_back_to_the_enterprise_domain_then_the_individual_endpoint() {
    let mut extra = Map::new();
    extra.insert(
        "enterpriseUrl".to_string(),
        json!("https://company.ghe.com"),
    );
    let enterprise = notagent_ai::auth::types::OAuthCredential {
        access: "no-proxy-ep".to_string(),
        refresh: "r".to_string(),
        expires: 0,
        extra,
    };
    let auth = github_copilot::github_copilot_oauth()
        .to_auth(enterprise)
        .await
        .expect("auth");
    assert_eq!(
        auth.base_url.as_deref(),
        Some("https://copilot-api.company.ghe.com")
    );

    let individual = notagent_ai::auth::types::OAuthCredential {
        access: "no-proxy-ep".to_string(),
        refresh: "r".to_string(),
        expires: 0,
        extra: Default::default(),
    };
    let auth = github_copilot::github_copilot_oauth()
        .to_auth(individual)
        .await
        .expect("auth");
    assert_eq!(
        auth.base_url.as_deref(),
        Some("https://api.individual.githubcopilot.com")
    );
}

#[test]
fn copilot_model_lists_respect_picker_and_policy_state() {
    let raw = json!({"data": [
        {"id": "picker", "model_picker_enabled": true, "policy": {"state": "enabled"}},
        {"id": "disabled-policy", "model_picker_enabled": true, "policy": {"state": "disabled"}},
        {"id": "policy-only", "model_picker_enabled": false, "policy": {"state": "enabled"}},
        {"id": "no-tools", "model_picker_enabled": true, "capabilities": {"supports": {"tool_calls": false}}}
    ]});
    // Picker entries win when there are any.
    assert_eq!(
        github_copilot::parse_available_copilot_model_ids(&raw, true).unwrap(),
        vec!["picker".to_string()]
    );

    // Without picker entries the fallback applies only where it is allowed.
    let policy_only = json!({"data": [{"id": "policy-only", "model_picker_enabled": false, "policy": {"state": "enabled"}}]});
    assert_eq!(
        github_copilot::parse_available_copilot_model_ids(&policy_only, true).unwrap(),
        vec!["policy-only".to_string()]
    );
    assert!(
        github_copilot::parse_available_copilot_model_ids(&policy_only, false)
            .unwrap()
            .is_empty()
    );

    assert!(github_copilot::parse_available_copilot_model_ids(&json!({}), true).is_err());
}

#[test]
fn copilot_normalizes_enterprise_domains() {
    assert_eq!(
        github_copilot::normalize_domain("https://company.ghe.com/path"),
        Some("company.ghe.com".to_string())
    );
    assert_eq!(
        github_copilot::normalize_domain("company.ghe.com"),
        Some("company.ghe.com".to_string())
    );
    assert_eq!(github_copilot::normalize_domain("  "), None);
}

#[test]
fn copilot_poll_responses_are_classified() {
    assert!(matches!(
        github_copilot::classify_poll_response(&body(json!({"access_token": "gho_token"}))),
        DeviceCodePollResult::Complete { .. }
    ));
    assert!(matches!(
        github_copilot::classify_poll_response(&body(json!({"error": "authorization_pending"}))),
        DeviceCodePollResult::Pending
    ));
    assert!(matches!(
        github_copilot::classify_poll_response(&body(json!({"error": "slow_down", "interval": 10}))),
        DeviceCodePollResult::SlowDown { interval_seconds: Some(interval) } if interval == 10.0
    ));
    assert!(matches!(
        github_copilot::classify_poll_response(&body(json!({"error": "access_denied"}))),
        DeviceCodePollResult::Failed { .. }
    ));
}

#[test]
fn codex_extracts_the_account_id_from_the_jwt_claim() {
    use base64::Engine;
    let payload = json!({"https://api.openai.com/auth": {"chatgpt_account_id": "acct_123"}});
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
    let token = format!("header.{encoded}.signature");

    assert_eq!(
        openai_codex::account_id(&token),
        Some("acct_123".to_string())
    );
    assert!(openai_codex::decode_jwt(&token).is_some());
    // Malformed tokens yield nothing instead of failing.
    assert_eq!(openai_codex::account_id("not-a-jwt"), None);
    assert_eq!(openai_codex::decode_jwt("a.b"), None);

    // A token without the claim cannot become a credential.
    let empty = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json!({}).to_string());
    assert!(
        openai_codex::credentials_from_token(format!("h.{empty}.s"), "r".to_string(), 0).is_err(),
        "the account id is mandatory"
    );
}

#[test]
fn codex_token_responses_require_all_fields() {
    let complete = body(json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600}));
    let (access, refresh, expires) =
        openai_codex::read_token_response(&complete, "exchange").expect("parsed");
    assert_eq!(access, "a");
    assert_eq!(refresh, "r");
    assert!(expires > notagent_ai::auth::resolve::now_ms());

    for incomplete in [
        json!({"refresh_token": "r", "expires_in": 1}),
        json!({"access_token": "a", "expires_in": 1}),
        json!({"access_token": "a", "refresh_token": "r"}),
    ] {
        assert!(openai_codex::read_token_response(&body(incomplete), "exchange").is_err());
    }
}

#[test]
fn anthropic_parses_every_manual_input_shape() {
    assert_eq!(
        anthropic::parse_authorization_input("http://localhost:53692/callback?code=abc&state=xyz"),
        (Some("abc".to_string()), Some("xyz".to_string()))
    );
    assert_eq!(
        anthropic::parse_authorization_input("abc#xyz"),
        (Some("abc".to_string()), Some("xyz".to_string()))
    );
    assert_eq!(
        anthropic::parse_authorization_input("code=abc&state=xyz"),
        (Some("abc".to_string()), Some("xyz".to_string()))
    );
    assert_eq!(
        anthropic::parse_authorization_input("bare-code"),
        (Some("bare-code".to_string()), None)
    );
    assert_eq!(anthropic::parse_authorization_input("   "), (None, None));
}

#[test]
fn xai_parses_device_codes_and_token_responses() {
    let device = xai::parse_device_code(&body(json!({
        "device_code": "d", "user_code": "u",
        "verification_uri": "https://x.ai/device", "expires_in": 600, "interval": 5
    })))
    .expect("device code");
    assert_eq!(device.device_code, "d");
    assert_eq!(device.interval_seconds, Some(5.0));

    // Non-https verification URIs are rejected.
    assert!(
        xai::parse_device_code(&body(json!({
            "device_code": "d", "user_code": "u", "verification_uri": "http://x.ai", "expires_in": 600
        })))
        .is_err()
    );

    // A refresh may omit refresh_token; the previous one is kept.
    let refreshed = xai::credentials_from_token_response(
        &body(json!({"access_token": "new", "expires_in": 3600})),
        Some("old-refresh"),
    )
    .expect("credential");
    assert_eq!(refreshed.access, "new");
    assert_eq!(refreshed.refresh, "old-refresh");

    // Without a previous token it is mandatory.
    assert!(
        xai::credentials_from_token_response(&body(json!({"access_token": "new"})), None).is_err()
    );
}

#[test]
fn xai_poll_responses_are_classified() {
    use notagent_ai::auth::oauth::http::OAuthHttpResponse;
    let response = |status: u16, ok: bool, value: Value| OAuthHttpResponse {
        ok,
        status,
        body: body(value),
    };

    assert!(matches!(
        xai::classify_poll_response(&response(
            400,
            false,
            json!({"error": "authorization_pending"})
        )),
        DeviceCodePollResult::Pending
    ));
    assert!(matches!(
        xai::classify_poll_response(&response(400, false, json!({"error": "slow_down", "interval": 8}))),
        DeviceCodePollResult::SlowDown { interval_seconds: Some(interval) } if interval == 8.0
    ));
    match xai::classify_poll_response(&response(400, false, json!({"error": "expired_token"}))) {
        DeviceCodePollResult::Failed { message } => assert_eq!(message, "xAI device code expired"),
        _ => panic!("expected a failure"),
    }
    match xai::classify_poll_response(&response(400, false, json!({"error": "access_denied"}))) {
        DeviceCodePollResult::Failed { message } => {
            assert_eq!(message, "xAI device authorization was denied")
        }
        _ => panic!("expected a failure"),
    }
}

#[test]
fn kimi_parses_device_authorizations_and_token_responses() {
    let device = kimi_coding::parse_device_authorization(&body(json!({
        "device_code": "d", "user_code": "u",
        "verification_uri": "https://auth.kimi.com/device",
        "verification_uri_complete": "https://auth.kimi.com/device?code=u"
    })))
    .expect("device authorization");
    // Missing interval and expiry fall back to the documented defaults.
    assert_eq!(device.interval_seconds, 5.0);
    assert_eq!(device.expires_in_seconds, 900.0);

    // A non-http verification URI is rejected.
    assert!(
        kimi_coding::parse_device_authorization(&body(json!({
            "device_code": "d", "user_code": "u",
            "verification_uri": "ftp://auth.kimi.com",
            "verification_uri_complete": "https://auth.kimi.com"
        })))
        .is_err()
    );

    assert!(
        kimi_coding::parse_token_response(&body(json!({"access_token": "a"})), "poll").is_err()
    );
    let credential = kimi_coding::parse_token_response(
        &body(json!({"access_token": "a", "refresh_token": "r", "expires_in": 60})),
        "poll",
    )
    .expect("credential");
    assert_eq!(credential.access, "a");
}

#[test]
fn kimi_poll_responses_are_classified() {
    assert!(matches!(
        kimi_coding::classify_poll_response(
            200,
            &body(json!({"access_token": "a", "refresh_token": "r", "expires_in": 60}))
        ),
        DeviceCodePollResult::Complete { .. }
    ));
    assert!(matches!(
        kimi_coding::classify_poll_response(400, &body(json!({"error": "authorization_pending"}))),
        DeviceCodePollResult::Pending
    ));
    assert!(matches!(
        kimi_coding::classify_poll_response(400, &body(json!({"error": "slow_down", "interval": 7}))),
        DeviceCodePollResult::SlowDown { interval_seconds: Some(interval) } if interval == 7.0
    ));
    match kimi_coding::classify_poll_response(400, &body(json!({"error": "expired_token"}))) {
        DeviceCodePollResult::Failed { message } => assert!(message.contains("expired")),
        _ => panic!("expected a failure"),
    }
    // A 5xx is a hard failure, not a pending poll.
    assert!(matches!(
        kimi_coding::classify_poll_response(503, &Map::new()),
        DeviceCodePollResult::Failed { .. }
    ));
}

#[test]
fn openrouter_parses_manual_inputs() {
    use notagent_ai::auth::oauth::openrouter::parse_authorization_input;
    assert_eq!(
        parse_authorization_input("http://127.0.0.1:1234/cb?code=abc"),
        Some("abc".to_string())
    );
    assert_eq!(
        parse_authorization_input("code=abc"),
        Some("abc".to_string())
    );
    assert_eq!(parse_authorization_input("abc"), Some("abc".to_string()));
    assert_eq!(parse_authorization_input(" "), None);
}

#[test]
fn radius_normalizes_its_gateway_url() {
    assert_eq!(
        radius::normalize_radius_gateway_url("https://gw.test/"),
        "https://gw.test"
    );
    assert_eq!(
        radius::normalize_radius_gateway_url("gw.test"),
        "https://gw.test"
    );
    // Only a leading `http(s)://` counts as a scheme, and the value is not trimmed:
    // `/^https?:\/\//iu.test(value)` fails for a padded input.
    assert_eq!(
        radius::normalize_radius_gateway_url("  http://gw.test  "),
        "https://  http://gw.test  "
    );
    assert_eq!(
        radius::normalize_radius_gateway_url("HTTP://gw.test//"),
        "HTTP://gw.test"
    );
}

#[test]
fn the_oauth_pages_escape_their_message() {
    use notagent_ai::auth::oauth::oauth_page::{oauth_error_html, oauth_success_html};
    let success = oauth_success_html("Done <script>alert(1)</script>");
    assert!(success.contains("&lt;script&gt;"), "the message is escaped");
    assert!(!success.contains("<script>alert"), "no raw markup survives");
    assert!(success.contains("<title>Authentication successful</title>"));

    let error = oauth_error_html("Failed", Some("detail & more"));
    assert!(error.contains("<title>Authentication failed</title>"));
    assert!(error.contains("detail &amp; more"));
    // Without details the block is omitted entirely.
    assert!(!oauth_error_html("Failed", None).contains(r#"<div class="details">"#));
}
