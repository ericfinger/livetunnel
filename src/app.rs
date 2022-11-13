use crate::Cli;

use confy::{get_configuration_file_path, load, store};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{
    validator::{Validation, ValueRequiredValidator},
    Confirm, CustomType, Editor, MultiSelect, Text, Password,
};
use lazy_static::lazy_static;
use openssh::{Session, SessionBuilder, Socket::TcpSocket};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use sha2::{Sha512, Digest};

use std::{
    env::current_dir,
    fmt::{Display, Formatter, Result},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::{exit, Command, Child},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::sleep,
    time::Duration,
};

lazy_static! {
    static ref INFO_TEMPLATE: ProgressStyle = ProgressStyle::with_template("ℹ {msg}").unwrap();
    static ref WARNING_TEMPLATE: ProgressStyle = ProgressStyle::with_template("❗{msg}").unwrap();
    static ref SUCCESS_TEMPLATE: ProgressStyle = ProgressStyle::with_template("✓ {msg}").unwrap();
}

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

    // users for auth:
    users: Vec<(String, String)>,
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
            println!("ℹ Starting setup assistant:");
            Self::build_config()
        } else {
            load("livetunnel", "livetunnel").unwrap()
        };

        if config.host.is_empty() {
            println!("❗Config file Invalid, starting setup assistant:");
            config = Self::build_config();
        }

        let directory = if let Some(dir) = cli.directory.clone() {
            if dir.exists() {
                dir
            } else {
                println!("❗Directory {:?} not found. Quitting.", dir);
                exit(1);
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
            let num_cmds = commands.len();
            println!(
                "ℹ Running {} command(s) before establishing SSH connection",
                num_cmds
            );

            for (i, (program, args)) in commands.iter().enumerate() {
                let pb = ProgressBar::new_spinner();
                pb.set_message(format!(
                    "[{}/{}] Running '{} {}'",
                    i + 1,
                    num_cmds,
                    program,
                    args
                ));
                pb.enable_steady_tick(Duration::from_millis(20));

                let mut child_process = Command::new(program);
                for arg in args.split(' ') {
                    child_process.arg(arg);
                }

                let output = match child_process.output() {
                    Ok(output) => output,
                    Err(err) => {
                        pb.set_style(WARNING_TEMPLATE.clone());
                        pb.tick();
                        pb.finish_with_message(format!(
                            "[{}/{}] Error: '{} {}' produced an Error: {}",
                            i + 1,
                            num_cmds,
                            program,
                            args,
                            err
                        ));
                        continue;
                    }
                };

                if !output.status.success() {
                    pb.set_style(WARNING_TEMPLATE.clone());
                    pb.tick();
                    pb.finish_with_message(format!(
                        "[{}/{}] Error: '{} {}' exited with {}: '{:?}'",
                        i + 1,
                        num_cmds,
                        program,
                        args,
                        output.status,
                        output
                    ));
                    continue;
                }

                pb.set_style(SUCCESS_TEMPLATE.clone());
                pb.tick();
                pb.finish_with_message(format!(
                    "[{}/{}] Done: '{} {}'",
                    i + 1,
                    num_cmds,
                    program,
                    args
                ));
            }
        }

        let pb = ProgressBar::new_spinner();
        pb.set_message(format!("Connecting to '{}' via SSH", config.host));
        pb.enable_steady_tick(Duration::from_millis(20));

        // Connect to SSH:
        let session = match runtime.block_on(session_builder.connect(&config.host)) {
            Ok(session) => session,
            Err(error) => panic!("Couldn't establish SSH connection: {:?}", error),
        };

        pb.set_style(SUCCESS_TEMPLATE.clone());
        pb.tick();
        pb.finish_with_message(format!("Connected to '{}' via SSH", config.host));

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

    pub fn run(&mut self) -> Child {

        if self.cli.secure {
            if self.config.users.is_empty() {
                println!("ℹ Secure sharing selected, but no User(s) set in config. Please add one now:");
                self.config.users = App::add_users();
            } else {
                let add_users = Confirm::new("ℹ Secure sharing selected. Do you want to add new users?")
                    .with_default(false)
                    .prompt()
                    .unwrap();

                if add_users {
                    let mut new_users = App::add_users();
                    self.config.users.append(&mut new_users);
                }

            }
        }

        let pb = ProgressBar::new_spinner();
        pb.set_message(format!(
            "Starting port-forward from local Port {} to remote Port {} via SSH",
            self.config.local_port, self.config.remote_port
        ));
        pb.enable_steady_tick(Duration::from_millis(20));

        let local_socket = TcpSocket(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            self.config.local_port,
        ));
        let remote_socket = TcpSocket(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            self.config.remote_port,
        ));

        self.runtime
            .block_on(self.session.request_port_forward(
                openssh::ForwardType::Remote,
                remote_socket,
                local_socket,
            ))
            .unwrap();

        pb.set_style(SUCCESS_TEMPLATE.clone());
        pb.tick();
        pb.finish_with_message(format!(
            "Started port-forward from local Port {} to remote Port {} via SSH",
            self.config.local_port, self.config.remote_port
        ));

        let mp = MultiProgress::new();
        let pb_forward = mp.add(ProgressBar::new_spinner());
        pb_forward.set_message(format!(
            "Forwarding local Port {} to remote Port {} via SSH",
            self.config.local_port, self.config.remote_port
        ));
        pb_forward.enable_steady_tick(Duration::from_millis(20));

        let pb_serve = mp.add(ProgressBar::new_spinner());
        pb_serve.set_message(format!(
            "Starting miniserve to serve content from '{}' on local Port '{}'",
            self.directory.display(),
            self.config.local_port
        ));
        pb_serve.enable_steady_tick(Duration::from_millis(20));

        let mut miniserve = Command::new("miniserve");

        // We don't care about miniserve's in-/output:
        miniserve.stdin(std::process::Stdio::null());
        miniserve.stdout(std::process::Stdio::null());
        miniserve.stderr(std::process::Stdio::null());

        // -H = show hidden files
        // -i = which network interface to use
        // -p port
        miniserve.args(["-H", "-i", "127.0.0.1", "-p", &self.config.local_port.to_string()]);

        if self.cli.secure {
            for (user, pw) in &self.config.users {
                miniserve.args(["-a", &format!("{}:sha512:{}", user, pw)]);
            }
        }

        miniserve.arg(&self.directory);

        let mut miniserve_handle = match miniserve.spawn() {
            Ok(handle) => handle,
            Err(_err) => panic!("Couldn't spawn miniserve"),
        };

        pb_serve.set_message(format!("miniserve successfully started. Serving content from '{}' on local Port '{}'",
            self.directory.display(),
            self.config.local_port
        ));

        let pb_exit_info = mp.add(ProgressBar::new(42));
        pb_exit_info.set_style(INFO_TEMPLATE.clone());
        pb_exit_info.set_message("Press CTRL+C to exit");

        loop {
            if self.runtime.block_on(self.session.check()).is_err() {
                pb_forward.set_style(WARNING_TEMPLATE.clone());
                pb_forward.tick();
                pb_forward.finish_with_message("SSH Forward died! Closing livetunnel.");
                self.should_end.store(true, Ordering::SeqCst);
                // TODO: Give option to reconnect
            };

            match miniserve_handle.try_wait() {
                Ok(status) => {
                    if status.is_some() {
                        if !status.unwrap().success() {
                            pb_serve.set_style(WARNING_TEMPLATE.clone());
                            pb_serve.tick();
                            pb_serve.finish_with_message(format!("miniserve exited unexpectantly {:?}", status));
                            // TODO: Give user option to restart/close
                        }
                    }
                },
                Err(err) => {
                    pb_serve.set_style(WARNING_TEMPLATE.clone());
                    pb_serve.tick();
                    pb_serve.finish_with_message(format!("miniserve died: {err}"));
                    // TODO: Give user option to restart/close
                }
            }

            if self.should_end.load(Ordering::SeqCst) {
                pb_forward.set_style(SUCCESS_TEMPLATE.clone());
                pb_forward.tick();
                pb_forward.finish();

                pb_serve.set_style(SUCCESS_TEMPLATE.clone());
                pb_serve.tick();
                pb_serve.finish();

                pb_exit_info.finish_and_clear();

                return miniserve_handle;
            }

            sleep(Duration::from_secs(1));
        }
    }

    pub fn close(self, mut miniserve_handle: Child) {
        let mp = MultiProgress::new();
        let pb_close = mp.add(ProgressBar::new_spinner());
        pb_close.set_message("Closing livetunnel");
        pb_close.enable_steady_tick(Duration::from_millis(20));
        sleep(Duration::from_secs(1));

        let steps = 2;

        let pb_ssh = mp.add(ProgressBar::new_spinner());
        pb_ssh.set_message(format!("[{}/{}] Closing SSH connection", 1, steps));
        pb_ssh.enable_steady_tick(Duration::from_millis(20));

        self.runtime.block_on(self.session.close()).unwrap();

        pb_ssh.set_style(SUCCESS_TEMPLATE.clone());
        pb_ssh.tick();
        pb_ssh.finish_with_message(format!("[{}/{}] Closed SSH connection", 1, steps));

        let pb_miniserve = mp.add(ProgressBar::new_spinner());
        pb_miniserve.set_message(format!("[{}/{}] Closing miniserve", 2, steps));
        pb_miniserve.enable_steady_tick(Duration::from_millis(20));

        if miniserve_handle.kill().is_ok() {
            // miniserve should already be killed by CTRL-C:
            // https://unix.stackexchange.com/questions/149741/why-is-sigint-not-propagated-to-child-process-when-sent-to-its-parent-process/149756#149756
            // TODO: Logging?
        }

        if let Err(err) = miniserve_handle.wait() {
            pb_miniserve.set_style(WARNING_TEMPLATE.clone());
            pb_miniserve.tick();
            pb_miniserve.finish_with_message(format!("Could not close miniserve: {err}"));
        } else {
            pb_miniserve.set_style(SUCCESS_TEMPLATE.clone());
            pb_miniserve.tick();
            pb_miniserve.finish_with_message(format!("[{}/{}] Successfully exited miniserve", 2, steps));
        }

        sleep(Duration::from_secs(1));
        pb_close.set_style(SUCCESS_TEMPLATE.clone());
        pb_close.tick();
        pb_close.finish_with_message("Successfully closed livetunnel");
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
                Text::new("SSH Keyfile:")
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

        let user_choice = Confirm::new("Do you want to add Users for secure sharing now? (You can always add users later when using the -s option)")
            .with_default(false)
            .prompt()
            .unwrap();

        let mut users = Vec::new();
        if user_choice {
            loop {
                let mut hasher = Sha512::new();
    
                let user = Text::new("Username:")
                    .with_validator(ValueRequiredValidator::default())
                    .prompt()
                    .unwrap();
    
                let password = Password::new("Password:")
                    .with_validator(ValueRequiredValidator::default())
                    .prompt()
                    .unwrap();
    
                hasher.update(password);
                users.push((String::from(user), format!("{:x}", hasher.finalize())));

                let stop = Confirm::new("Do you want to add another User?")
                    .with_default(false)
                    .prompt()
                    .unwrap();

                if !stop {
                    break;
                }
            }
        }

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
            users,
        };

        store("livetunnel", "livetunnel", &config).unwrap();

        config
    }

    fn add_users() -> Vec<(String, String)> {
        let mut hasher = Sha512::new();
        let mut users = Vec::new();

        loop {
            let user = Text::new("Username:")
                .with_validator(ValueRequiredValidator::default())
                .prompt()
                .unwrap();

            let password = Password::new("Password:")
                .with_validator(ValueRequiredValidator::default())
                .prompt()
                .unwrap();

            hasher.update(password);
            users.push((String::from(user), format!("{:x}", hasher.finalize_reset())));

            let stop = Confirm::new("Do you want to add another User?")
                .with_default(false)
                .prompt()
                .unwrap();

            if !stop {
                break;
            }
        }

        users
    }
}
