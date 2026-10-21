use clap::Parser;
use wasix_eth_app::app::App;
use wasix_eth_app::cli::{self, Commands};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("[PANIC] {}", panic_info);
    }));

    let mut args = cli::Args::parse();

    match &args.command {
        Some(Commands::Init { common }) => {
            args.common = common.clone();
            let app = App::builder()
                .with_config(args.clone())
                .build_init()
                .await?;
            app.init().await?;
        }
        Some(Commands::Import { common }) => {
            args.common = common.clone();
            let app = App::builder()
                .with_config(args.clone())
                .build_import()
                .await?;
            app.import().await?;
        }
        Some(Commands::Run { common }) => {
            args.common = common.clone();
            let app = App::builder()
                .with_config(args.clone())
                .build()
                .await?;
            app.run().await?;
        }
        None => {
            let app = App::builder()
                .with_config(args.clone())
                .build()
                .await?;
            app.run().await?;
        }
    }

    Ok(())
}
