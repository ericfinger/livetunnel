mod app;

use crate::app::App;

use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

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

    let end: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let end_app = end.clone();

    ctrlc::set_handler(move || {
        end.store(true, Ordering::Relaxed);
    })
    .unwrap();

    let mut app = App::new(cli, end_app);

    app.run();
    app.close();
}
