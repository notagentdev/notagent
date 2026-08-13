//! Cloudflare endpoint placeholders.
//!
//! 1:1 port of `packages/ai/src/providers/cloudflare-stream.ts` (28 LOC).

use std::sync::Arc;

use crate::types::{
    Context, Model, ProviderEnv, ProviderStreams, SimpleStreamOptions, StreamOptions,
};
use crate::utils::event_stream::AssistantMessageEventStream;

const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

/// `resolveCloudflareModel(model, env)`
pub fn resolve_cloudflare_model(model: &Model, env: Option<&ProviderEnv>) -> Model {
    let Some(env) = env else {
        return model.clone();
    };
    let base_url = model
        .base_url
        .replace(
            &format!("{{{CLOUDFLARE_ACCOUNT_ID}}}"),
            &env.get(CLOUDFLARE_ACCOUNT_ID)
                .cloned()
                .unwrap_or_else(|| format!("{{{CLOUDFLARE_ACCOUNT_ID}}}")),
        )
        .replace(
            &format!("{{{CLOUDFLARE_GATEWAY_ID}}}"),
            &env.get(CLOUDFLARE_GATEWAY_ID)
                .cloned()
                .unwrap_or_else(|| format!("{{{CLOUDFLARE_GATEWAY_ID}}}")),
        );
    if base_url == model.base_url {
        return model.clone();
    }
    Model {
        base_url,
        ..model.clone()
    }
}

/// `cloudflareStreams(streams)` — materializes the account/gateway placeholders from the
/// resolved provider env before dispatch. Like the TS wrapper it exposes only
/// `stream`/`streamSimple`, so a wrapped api never advertises deferred support.
pub struct CloudflareStreams {
    streams: Arc<dyn ProviderStreams>,
}

/// `cloudflareStreams(streams)`
pub fn cloudflare_streams(streams: Arc<dyn ProviderStreams>) -> Arc<CloudflareStreams> {
    Arc::new(CloudflareStreams { streams })
}

impl ProviderStreams for CloudflareStreams {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let model = resolve_cloudflare_model(
            model,
            options
                .as_ref()
                .and_then(|options| options.base.env.as_ref()),
        );
        self.streams.stream(&model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let model = resolve_cloudflare_model(
            model,
            options
                .as_ref()
                .and_then(|options| options.base.base.env.as_ref()),
        );
        self.streams.stream_simple(&model, context, options)
    }
}
