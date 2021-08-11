use serde::Deserialize;
use config::Config;

use anyhow::Result;

#[derive(Default, Debug, Deserialize)]
pub struct ServerConfig {
    pub bind_addresses: Vec<String>,
    pub trace: Option<TraceConfig>,
}

#[derive(Default, Debug, Deserialize)]
pub struct TraceConfig {
    pub service_name: String,
    pub agent_endpoint: String,
}

impl ServerConfig {
    pub fn new<P: AsRef<str>>(path: P) -> Result<ServerConfig> {
        let mut config = Config::default();
        config.set_default("bind_addresses", vec!["0.0.0.0:5353"])?;
        config.merge(config::File::with_name(path.as_ref()))?;

        Ok(config.try_into()?)
    }
}