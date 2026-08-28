use notagent_ai::auth::types::AuthOperationOptions;
use notagent_ai::types::Model;
use notagent_tui::fuzzy::fuzzy_filter;
use tokio_util::sync::CancellationToken;

use crate::core::auth_guidance::format_no_models_available_message;
use crate::core::model_runtime::ModelRuntime;
use crate::core::output_guard::console_log;
use crate::utils::chalk::yellow;

/// `200000` → `200K`, `1000000` → `1M`.
fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let millions = count as f64 / 1_000_000.0;
        return if millions.fract() == 0.0 {
            format!("{millions}M")
        } else {
            format!("{millions:.1}M")
        };
    }
    if count >= 1_000 {
        let thousands = count as f64 / 1_000.0;
        return if thousands.fract() == 0.0 {
            format!("{thousands}K")
        } else {
            format!("{thousands:.1}K")
        };
    }
    count.to_string()
}

struct Row {
    provider: String,
    model: String,
    context: String,
    max_out: String,
    thinking: String,
    images: String,
}

fn pad_end(text: &str, width: usize) -> String {
    // `String.prototype.padEnd` counts UTF-16 units; model ids and provider ids
    // are ASCII, so counting characters is the same thing here.
    let length = text.chars().count();
    if length >= width {
        return text.to_owned();
    }
    format!("{text}{}", " ".repeat(width - length))
}

/// Prints the model table, filtered by `search_pattern` when given.
pub async fn list_models(
    model_runtime: &ModelRuntime,
    search_pattern: Option<&str>,
    signal: Option<CancellationToken>,
) {
    if let Some(load_error) = model_runtime.get_error() {
        eprintln!(
            "{}",
            yellow(&format!(
                "Warning: errors loading models.json:\n{load_error}"
            ))
        );
    }

    // the process; here it degrades to the empty list, which prints the same
    // "no models" guidance the empty catalog prints.
    let models: Vec<Model> = model_runtime
        .get_available(None, Some(AuthOperationOptions { signal }))
        .await
        .unwrap_or_default();

    if models.is_empty() {
        console_log(&format_no_models_available_message());
        return;
    }

    let mut filtered_models = match search_pattern {
        Some(pattern) => fuzzy_filter(&models, pattern, |model| {
            format!("{} {}", model.provider, model.id)
        }),
        None => models,
    };

    if filtered_models.is_empty() {
        console_log(&format!(
            "No models matching \"{}\"",
            search_pattern.unwrap_or_default()
        ));
        return;
    }

    filtered_models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.id.cmp(&right.id))
    });

    let rows: Vec<Row> = filtered_models
        .iter()
        .map(|model| Row {
            provider: model.provider.clone(),
            model: model.id.clone(),
            context: format_token_count(model.context_window),
            max_out: format_token_count(model.max_tokens),
            thinking: if model.reasoning { "yes" } else { "no" }.to_owned(),
            images: if model
                .input
                .iter()
                .any(|input| input == &notagent_ai::types::Modality::Image)
            {
                "yes"
            } else {
                "no"
            }
            .to_owned(),
        })
        .collect();

    let headers = [
        "provider", "model", "context", "max-out", "thinking", "images",
    ];
    let cells: Vec<[&str; 6]> = rows
        .iter()
        .map(|row| {
            [
                row.provider.as_str(),
                row.model.as_str(),
                row.context.as_str(),
                row.max_out.as_str(),
                row.thinking.as_str(),
                row.images.as_str(),
            ]
        })
        .collect();

    let mut widths = [0usize; 6];
    for (index, header) in headers.iter().enumerate() {
        widths[index] = header.chars().count();
        for row in &cells {
            widths[index] = widths[index].max(row[index].chars().count());
        }
    }

    let header_line = headers
        .iter()
        .enumerate()
        .map(|(index, header)| pad_end(header, widths[index]))
        .collect::<Vec<_>>()
        .join("  ");
    console_log(&header_line);

    for row in &cells {
        let line = row
            .iter()
            .enumerate()
            .map(|(index, cell)| pad_end(cell, widths[index]))
            .collect::<Vec<_>>()
            .join("  ");
        console_log(&line);
    }
}
