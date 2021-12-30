use crate::config::ConfigBuilder;
use std::env;

mod certificate;
mod config;
mod server;

pub use config::Config;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    let mut cfg_builder = ConfigBuilder::new();

    let config = match cfg_builder.parse(&args) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("{}\n\n{}", err, cfg_builder.get_usage());
            return;
        }
    };

    match server::start(config).await {
        Ok(()) => {}
        Err(err) => eprintln!("{}", err),
    }
}
