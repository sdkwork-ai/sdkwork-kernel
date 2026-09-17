//! Trace correlation and optional OpenTelemetry export (`OBSERVABILITY_SPEC.md`).

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::ServerConfig;

/// Initialize global tracing with optional OTLP export when configured.
pub fn init_tracing(config: &ServerConfig) -> anyhow::Result<()> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    let fmt_layer = tracing_subscriber::fmt::layer();

    #[cfg(feature = "observability-otel")]
    {
        if let Some(endpoint) = config
            .otel_exporter_otlp_endpoint
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return init_with_otlp(env_filter, endpoint);
        }
    }

    let _ = config;
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();
    Ok(())
}

#[cfg(feature = "observability-otel")]
fn init_with_otlp(env_filter: EnvFilter, endpoint: &str) -> anyhow::Result<()> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()?;

    // `Resource::builder()` carries the default detectors (OTEL_RESOURCE_ATTRIBUTES
    // plus the telemetry SDK attributes); `with_service_name` pins service.name.
    let resource = Resource::builder()
        .with_service_name("sdkwork-agent-server")
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("sdkwork-agent-server");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let fmt_layer = tracing_subscriber::fmt::layer();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    std::mem::forget(provider);
    Ok(())
}

/// Parse W3C `traceparent` (`00-<trace-id>-<parent-id>-<flags>`).
pub fn trace_id_from_traceparent(value: &str) -> Option<String> {
    let mut parts = value.split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let _parent_id = parts.next()?;
    let _flags = parts.next()?;
    if version != "00" || trace_id.len() != 32 {
        return None;
    }
    if !trace_id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some(trace_id.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_traceparent() {
        let trace_id =
            trace_id_from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
                .expect("traceparent should parse");
        assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn rejects_invalid_traceparent() {
        assert!(trace_id_from_traceparent("invalid").is_none());
    }
}
