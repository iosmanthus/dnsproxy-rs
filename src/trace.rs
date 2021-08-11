use anyhow::{Result};
use tracing_subscriber::prelude::*;
use tracing_subscriber::Registry;

use crate::config::TraceConfig;

pub(crate) fn init(config: Option<TraceConfig>) -> Result<()> {
    if let Some(config) = config {
        let tracer = opentelemetry_jaeger::new_pipeline()
            .with_agent_endpoint(config.agent_endpoint)
            .with_service_name(config.service_name)
            .install_simple()?;
        // Create a tracing layer with the configured tracer
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = Registry::default().with(telemetry);
        tracing::subscriber::set_global_default(subscriber)?;
    }
    Ok(())
}