use crate::Cli;
use confy::{get_configuration_file_path, load, store};
use inquire::{
    validator::{Validation, ValueRequiredValidator},
    Confirm, CustomType, Editor, MultiSelect, Text,
};
use openssh::{Session, SessionBuilder};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use std::{
    env::current_dir,
    fmt::{Display, Formatter, Result},
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

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

enum OptionalFeatures {
    CmdBefore,
    CmdAfter,
    JumpHosts,
}

impl Display for OptionalFeatures {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            OptionalFeatures::CmdBefore => write!(
                f,
                "Run Command (locally) before establishing SSH connection"
            ),
            OptionalFeatures::CmdAfter => write!(
                f,
                "Run command (remotely) after establishing SSH connection"
            ),
            OptionalFeatures::JumpHosts => write!(f, "Use SSH jump-hosts"),
        }
    }
}

#[allow(dead_code)]
pub struct App {
    pub cli: Cli,
    config: Config,
    directory: PathBuf,
    runtime: Runtime,
    session: Session,
    pub should_end: Arc<AtomicBool>,
}

impl App {
    pub fn new(cli: Cli, end: Arc<AtomicBool>) -> Self {
        let mut config = if cli.reconfigure
            || get_configuration_file_path("livetunnel", "livetunnel").is_err()
        {
            Self::build_config()
        } else {
            load("livetunnel", "livetunnel").unwrap()
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
            current_dir().unwrap()
        };

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

        if let Some(ref commands) = config.before_commands {
            for (program, args) in commands {
                let mut child_process = Command::new(program);
                for arg in args.split(' ') {
                    child_process.arg(arg);
                }
                let output = child_process.output().unwrap();
                if !output.status.success() {
                    panic!(
                        "Program '{}' exited with exit status {}: {:#?}",
                        program, output.status, output
                    );
                }
            }
        }

        // Connect to SSH:
        let session = match runtime.block_on(session_builder.connect(&config.host)) {
            Ok(session) => session,
            Err(error) => panic!("Couldn't establish SSH connection: {:?}", error),
        };

        // TODO: Execute after commands

        App {
            cli,
            config,
            directory,
            runtime,
            session,
            should_end: end,
        }
    }

    pub fn run(&mut self) {
        let local_socket = openssh::Socket::TcpSocket(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            self.config.local_port,
        ));
        let remote_socket = openssh::Socket::TcpSocket(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            self.config.remote_port,
        ));

        self.runtime
            .block_on(self.session.request_port_forward(
                openssh::ForwardType::Remote,
                remote_socket,
                local_socket,
            ))
            .unwrap();

        loop {
            if self.runtime.block_on(self.session.check()).is_err() {
                panic!("died");
            };

            if self.should_end.load(Ordering::SeqCst) {
                return;
            }

            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }

    pub fn close(self) {
        self.runtime.block_on(self.session.close()).unwrap();
        println!("Cleaned up. Byyyyeeeee");
    }

    fn build_config() -> Config {
        let optional_features = vec![
            OptionalFeatures::CmdBefore,
            OptionalFeatures::CmdAfter,
            OptionalFeatures::JumpHosts,
        ];

        let selection = MultiSelect::new(
            "Select which optional Features you'd like to use:",
            optional_features,
        )
        .with_vim_mode(true)
        .prompt()
        .unwrap();

        let host = Text::new("SSH Host:")
            .with_validator(ValueRequiredValidator::default())
            .prompt()
            .unwrap();

        let port = if Confirm::new("Set Port?")
            .with_default(false)
            .prompt()
            .unwrap()
        {
            Some(
                CustomType::<u16>::new("SSH Port:")
                    .with_default(22)
                    .with_error_message("Not a valid Port Number")
                    .prompt()
                    .unwrap(),
            )
        } else {
            None
        };

        let username = if Confirm::new("Set Username?")
            .with_default(false)
            .prompt()
            .unwrap()
        {
            Some(
                Text::new("SSH user:")
                    .with_validator(ValueRequiredValidator::default())
                    .with_default("root")
                    .prompt()
                    .unwrap(),
            )
        } else {
            None
        };

        let keyfile = if Confirm::new("Set Keyfile?")
            .with_default(false)
            .prompt()
            .unwrap()
        {
            Some(
                Text::new("SSH user:")
                    .with_validator(|input: &str| {
                        let path = PathBuf::from(input);
                        if path.exists() {
                            if path.is_file() {
                                Ok(Validation::Valid)
                            } else {
                                Ok(Validation::Invalid("Not a file".into()))
                            }
                        } else {
                            Ok(Validation::Invalid("The given file does not exist".into()))
                        }
                    })
                    .with_placeholder("~/.ssh/id_rsa")
                    .prompt()
                    .unwrap()
                    .into(),
            )
        } else {
            None
        };

        let remote_port = CustomType::<u16>::new("Remote Port to forward to:")
            .with_error_message("Not a valid Port Number")
            .prompt()
            .unwrap();

        let local_port = CustomType::<u16>::new("Local Port to host on / forward:")
            .with_default(3000)
            .with_error_message("Not a valid Port Number")
            .prompt()
            .unwrap();

        let mut before_cmd: Vec<(String, String)> = vec![];
        let mut after_cmd: Vec<(String, String)> = vec![];
        let mut jump_h: Vec<String> = vec![];

        for entry in selection {
            match entry {
                OptionalFeatures::CmdBefore => {
                    let cmd = Editor::new("Which commands should be run before making the SSH connection (One per line):")
                        .with_validator(ValueRequiredValidator::default())
                        .with_editor_command(std::ffi::OsStr::new("vim"))
                        .prompt();

                    if cmd.is_err() {
                        continue;
                    }

                    for line in cmd.unwrap().lines() {
                        let command = line.split_once(' ');
                        match command {
                            // (program) (Arguments)
                            Some(x) => before_cmd.push((String::from(x.0), String::from(x.1))),
                            None => before_cmd.push((String::from(line), String::new())),
                        }
                    }
                }

                OptionalFeatures::CmdAfter => {
                    let cmd = Editor::new("Which commands should be run (remotly) after making the SSH connection (One per line):")
                        .with_validator(ValueRequiredValidator::default())
                        .with_editor_command(std::ffi::OsStr::new("vim"))
                        .prompt();

                    if cmd.is_err() {
                        continue;
                    }

                    for line in cmd.unwrap().lines() {
                        let command = line.split_once(' ');
                        match command {
                            // (program) (Arguments)
                            Some(x) => after_cmd.push((String::from(x.0), String::from(x.1))),
                            None => after_cmd.push((String::from(line), String::new())),
                        }
                    }
                }

                OptionalFeatures::JumpHosts => {
                    let cmd = Editor::new("Please specify your List of Jump-Hosts (one per line):")
                        .with_validator(ValueRequiredValidator::default())
                        .with_editor_command(std::ffi::OsStr::new("vim"))
                        .prompt();

                    if cmd.is_err() {
                        continue;
                    }

                    for line in cmd.unwrap().lines() {
                        jump_h.push(String::from(line));
                    }
                }
            }
        }

        let config = Config {
            before_commands: if before_cmd.is_empty() {
                None
            } else {
                Some(before_cmd)
            },
            after_commands: if after_cmd.is_empty() {
                None
            } else {
                Some(after_cmd)
            },
            host,
            port,
            username,
            keyfile,
            jump_hosts: if jump_h.is_empty() {
                None
            } else {
                Some(jump_h)
            },
            local_port,
            remote_port,
        };

        store("livetunnel", "livetunnel", &config).unwrap();

        config
    }
}
