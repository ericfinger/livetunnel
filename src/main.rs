mod app;

use crate::app::App;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = "Tunnel your local files to your own Webserver"
)]
pub struct Cli {
    /// Reconfigure the app via the config assistant
    #[arg(long)]
    reconfigure: bool,

    /// Set a password for the hosted site
    #[arg(short, long)]
    secure: bool,

    /// Which directory to host (default: cwd)
    directory: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    let _app = App::new(cli);
}
