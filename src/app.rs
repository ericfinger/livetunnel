use crate::Cli;
use confy::{get_configuration_file_path, load};
use openssh::SessionBuilder;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use std::path::PathBuf;

#[derive(Default, Debug, Serialize, Deserialize)]
struct Config {
    // Commands that should be run locally before making the SSH-connection:
    before_commands: Option<Vec<(String, String)>>,
    // Commands that should be run remotely after making the SSH-connection:
    after_commands: Option<Vec<(String, String)>>,

    // SSH settings:
    host: String,
    port: Option<u16>,
    username: Option<String>,
    keyfile: Option<PathBuf>,
    jump_hosts: Option<Vec<String>>,

    // Port forwards:
    local_port: u16,
    remote_port: u16,
}

#[allow(dead_code)]
pub struct App {
    pub cli: Cli,
    config: Config,
    directory: PathBuf,
    runtime: Runtime,
    session: openssh::Session,
}

impl App {
    pub fn new(cli: Cli) -> Self {
        let mut config = if get_configuration_file_path("livetunnel", "livetunnel.conf").is_err() {
            Self::build_config()
        } else {
            load("livetunnel", "livetunnel.conf").unwrap()
        };

        if config.host.is_empty() {
            config = Self::build_config();
        }

        let directory = if let Some(dir) = cli.directory.clone() {
            if dir.exists() {
                dir
            } else {
                panic!("Directory {:?} not found.", dir);
            }
        } else {
            std::env::current_dir().unwrap()
        };

        // TODO: Probably switch to thread pool
        let runtime = Runtime::new().unwrap();

        // Build SSH Connection from config:
        let mut session_builder = SessionBuilder::default();
        if let Some(port) = config.port {
            session_builder.port(port);
        }

        if let Some(username) = config.username.clone() {
            session_builder.user(username);
        }

        if let Some(keyfile) = &config.keyfile {
            session_builder.keyfile(keyfile);
        }

        if let Some(jump_hosts) = &config.jump_hosts {
            session_builder.jump_hosts(jump_hosts);
        }

        // Connect to SSH:
        let session = match runtime.block_on(session_builder.connect(&config.host)) {
            Ok(session) => session,
            Err(error) => panic!("Couldn't establish SSH connection: {:?}", error),
        };

        // TODO: Webserver

        App {
            cli,
            config,
            directory,
            runtime,
            session,
        }
    }

    fn build_config() -> Config {
        todo!("use inquire to get config")

        // TODO: confy store
    }
}
