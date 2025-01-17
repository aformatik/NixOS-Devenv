#![feature(let_chains)]
#![feature(once_cell_try)]
#![deny(unused_crate_dependencies)]

use crate::{
    cli::{Cli, Cmd, CLI_ARGS},
    platform::{Driver, Machine, Store},
};
use clap::{CommandFactory, Parser};
use config::{git_url::GitUrl, CodchiConfig, MachineConfig};
use console::style;
use log::Level;
use logging::{set_progress_status, CodchiOutput};
use platform::{store_debug_shell, ConfigStatus, Host, MachineDriver};
use std::{
    env,
    panic::{self, PanicInfo},
    process::exit,
};
use util::{ResultExt, UtilExt};

pub mod cli;
pub mod config;
pub mod consts;
pub mod logging;
pub mod module;
pub mod platform;
pub mod tray;
pub mod util;

fn main() -> anyhow::Result<()> {
    panic::set_hook(Box::new(move |info: &PanicInfo<'_>| {
        let meta = human_panic::Metadata::new(
            env!("CARGO_PKG_NAME"),
            format!(
                "{} Commit: {}",
                env!("CARGO_PKG_VERSION"),
                env!("CODCHI_GIT_COMMIT")
            ),
        );
        let cause = get_panic_cause(info);
        let report_file = match human_panic::handle_dump(&meta, info) {
            Some(fp) => format!("{}", fp.display()),
            None => "<Failed to store file to disk>".to_owned(),
        };
        let msg = format!(
            r#"Well, this is embarrassing.

Codchi had a problem and crashed. It seems that the problem has to do with the following:

{cause}

Codchi is currently in beta and still under active development and testing. Sometimes it can help to execute the same command 2 or 3 times. In the worst case you should be able to export your machine files with `codchi tar <CODCHI_MACHINE> export.tar`.

To help us diagnose the problem you can send us a crash report.
We have generated a report file at "{report_file}". Submit an issue or email with the subject of "Codchi Crash Report" and include the report as an attachment.

To submit the crash report:

- Create an issue at <https://github.com/aformatik/codchi/issues>.
- For professional support contact aformatik: <https://www.aformatik.de/kontakt/> or <codchi[at]aformatik[dot]de>.

We take privacy very seriously - we don't perform any automated error collection. In order to improve the software, we rely on users like you to submit reports.

Thank you kindly!"#
        );
        eprintln!("{}", style(msg).red());
    }));

    let cli = Cli::parse();

    // process immediate commands
    if let Some(Cmd::Completion { shell }) = &cli.command {
        shell.generate(&mut Cli::command(), &mut std::io::stdout());
        exit(0);
    }

    logging::init(cli.verbose.log_level_filter())?;

    log::trace!("Started codchi with args: {:?}", cli);

    // preload config
    let cfg = CodchiConfig::get();

    if !matches!(cli.command, Some(Cmd::Tray {})) && cfg.tray.autostart {
        Driver::host()
            .start_tray(false)
            .trace_err("Failed starting codchi's tray")
            .ignore();
    }

    // process commands without the store commands
    match &cli.command {
        Some(Cmd::Tar { name, target_file }) => {
            progress_scope! {
                set_progress_status(format!("Exporting files of {name} to {target_file:?}..."));
                Machine::by_name(name, false)?.tar(target_file)?;
                log::info!("Success! Exported file system of machine {name} to {target_file:?}");
            }
            exit(0);
        }
        Some(Cmd::Store(store)) => match store {
            cli::StoreCmd::Debug => store_debug_shell()?,
            #[cfg(target_os = "windows")]
            cli::StoreCmd::Recover => {
                platform::store_recover()?;
                exit(0);
            }
        },
        _ => {}
    }

    CLI_ARGS
        .set(cli.clone())
        .expect("Only main is allowed to set CLI_ARGS.");

    let _ = Driver::store();

    // all other commands
    match &cli.command.unwrap_or(Cmd::Status {}) {
        Cmd::Status {} => Machine::list(true)?.print(cli.json),
        Cmd::Init {
            machine_name,
            url,
            input_options: options,
            module_paths,
        } => {
            (|| {
                let machine = module::init(
                    machine_name,
                    url.as_ref().map(GitUrl::from),
                    options,
                    module_paths,
                )?;
                if !options.no_build {
                    machine.build(true)?;
                    machine.run_init_script()?;
                    log::info!("Machine '{machine_name}' is ready! Use `codchi exec {machine_name}` to start it.")
                } else {
                    alert_dirty(machine);
                }
                anyhow::Ok(())
            })()
            .inspect_err(|_| {
                if !log::log_enabled!(Level::Debug) {
                    log::error!("Failed initializing machine '{machine_name}'. Removing leftovers...");
                    if let Ok(machine) = Machine::by_name(machine_name, false) {
                        machine.delete(true).ignore();
                    }
                }
            })?;
        }
        Cmd::Clone {
            machine_name,
            dir,
            url,
            input_options,
            module_paths,
            depth,
            single_branch,
            recurse_submodules,
            shallow_submodules,
            keep_remote,
        } => {
            (|| {
                let machine = module::clone(
                    machine_name,
                    GitUrl::from(url),
                    input_options,
                    module_paths,
                    dir,
                    depth,
                    single_branch,
                    recurse_submodules,
                    shallow_submodules,
                    keep_remote
                )?;
                machine.run_init_script()
            })()
            .inspect_err(|_| {
                if !log::log_enabled!(Level::Debug) {
                    log::error!("Failed cloning machine '{machine_name}'. Removing leftovers...");
                    if let Ok(machine) = Machine::by_name(machine_name, false) {
                        machine.delete(true).ignore();
                    }
                }
            })?;
            log::info!(
                "Machine '{machine_name}' is ready! Use `codchi exec {machine_name}` to start it."
            );
        }
        Cmd::Rebuild { no_update, name } => {
            Machine::by_name(name, true)?.build(*no_update)?;
            log::info!("Machine {name} rebuilt successfully!");
        }
        Cmd::Exec { name, cmd } => Machine::by_name(name, true)?.exec(cmd)?,
        Cmd::Delete {
            name,
            i_am_really_sure,
        } => Machine::by_name(name, true)?.delete(*i_am_really_sure)?,
        Cmd::Module(cmd) => match cmd {
            cli::ModuleCmd::List { name } => {
                let json = cli.json;
                let (_, cfg) = MachineConfig::open_existing(name, false)?;
                cfg.modules.print(json);
            }
            cli::ModuleCmd::Add {
                machine_name,
                url,
                options,
                module_paths,
            } => {
                let machine = module::add(machine_name, GitUrl::from(url), options, module_paths)?;
                if !options.no_build {
                    machine.build(true)?;
                } else {
                    alert_dirty(machine);
                }
            }
            cli::ModuleCmd::Set {
                machine_name,
                url,
                options,
                name,
                new_name,
                module_path,
            } => {
                let machine = module::set(
                    machine_name,
                    name,
                    options,
                    new_name,
                    module_path,
                    url.as_ref().map(GitUrl::from),
                )?;
                if !options.no_build {
                    machine.build(false)?;
                } else {
                    alert_dirty(machine);
                }
            }
            cli::ModuleCmd::Delete { name, module_name } => {
                alert_dirty(module::delete(name, module_name)?)
            }
        },
        Cmd::GC {
            delete_old,
            all,
            machines,
        } => Driver::store().gc(delete_old.map(|x| x.unwrap_or_default()), *all, machines)?,
        Cmd::Tray {} => tray::run()?,
        Cmd::Completion { .. } => unreachable!(),
        Cmd::Tar { .. } => unreachable!(),
        Cmd::Store(_) => unreachable!(),
    }
    if CodchiConfig::get().tray.autostart {
        Driver::host()
            .start_tray(true)
            .trace_err("Failed restarting tray")
            .ignore();
    }

    Ok(())
}

// Alerts the user if there was a change to the machine. Also restarts / updates tray
fn alert_dirty(machine: Machine) {
    match machine.config_status {
        ConfigStatus::NotInstalled => {
            println!(
                "{} is not installed yet. Install with `codchi rebuild {}`",
                machine.config.name, machine.config.name
            );
        }
        ConfigStatus::Modified => {
            println!(
                "{} was modified. Apply changes with `codchi rebuild {}`",
                machine.config.name, machine.config.name
            );
        }
        ConfigStatus::UpdatesAvailable => {
            println!(
                "{} has been updated upstream. Update with `codchi rebuild {}`",
                machine.config.name, machine.config.name
            );
        }
        ConfigStatus::UpToDate => {
            println!("Everything up to date!");
        }
    }
}

fn get_panic_cause(panic_info: &PanicInfo) -> String {
    #[cfg(feature = "nightly")]
    let message = panic_info.message().map(|m| format!("{}", m));

    #[cfg(not(feature = "nightly"))]
    let message = match (
        panic_info.payload().downcast_ref::<&str>(),
        panic_info.payload().downcast_ref::<String>(),
    ) {
        (Some(s), _) => Some(s.to_string()),
        (_, Some(s)) => Some(s.to_string()),
        (None, None) => None,
    };

    match message {
        Some(m) => m,
        None => "Unknown".into(),
    }

    // Note: The `None` case will almost NEVER happen. I couldn't find an immediately obvious way to make it occur.
    // Using unwrap(), panic!(), array[38924], etc. still provided some message.
    // I'd reckon we could twist a None out at some point, but I couldn't find anything common at all...
    //
    // Please let me know if you have some ideas which may neccessite other forms of handling.
}
