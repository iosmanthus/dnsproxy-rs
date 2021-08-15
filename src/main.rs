use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use trust_dns_server::ServerFuture;

use anyhow::Result;

use clap::{AppSettings, Clap};

mod cache;
mod config;
mod handler;
mod trace;

use crate::config::ServerConfig;
use crate::handler::{DnsProxy, Upstream};

#[derive(Clap, Debug)]
#[clap(version = "0.1.0", author = "iosmanthus. <myosmanthustree@gmail.com>")]
#[clap(setting = AppSettings::ColoredHelp)]
struct Opts {
    #[clap(short, long, default_value = "config.yaml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let config = ServerConfig::new(&opts.config)?;

    trace::init(config.trace)?;

    let mut server = ServerFuture::new(DnsProxy::new(vec![Box::new(Upstream::new(
        "1.1.1.1:53".parse()?,
        Duration::from_secs(5),
    ))])?);

    for addr in config.bind_addresses.iter() {
        server.register_listener(
            TcpListener::bind(addr.parse::<SocketAddr>()?).await?,
            Duration::from_secs(10),
        );
    }
    server.block_until_done().await?;
    Ok(())
}
