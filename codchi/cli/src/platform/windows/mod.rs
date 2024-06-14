mod host;
// mod util;
mod wsl;

pub use host::*;
use indicatif::ProgressBar;

use self::wsl::wsl_command;
use super::{
    private::Private, Driver, LinuxCommandTarget, LinuxPath, LinuxUser, Machine, MachineDriver,
    NixDriver, PlatformStatus, Store,
};
use crate::{
    cli::DEBUG,
    config::CodchiConfig,
    consts::{
        self, files,
        machine::{self, machine_name},
        PathExt, ToPath, CODCHI_FLAKE_URL, NIX_SYSTEM,
    },
    platform::CommandExt,
    util::ResultExt,
};
use anyhow::{Context, Result};
use std::{collections::HashMap, env, fs, path::PathBuf, thread, time::Duration};

pub const NIX_STORE_PACKAGE: &str = "store-wsl";
pub const NIXOS_DRIVER_NAME: &str = "wsl";

pub struct StoreImpl {}

// https://github.com/rust-lang/cargo/issues/1721

impl Store for StoreImpl {
    fn start_or_init_container(spinner: &mut ProgressBar, _: Private) -> Result<Self> {
        wsl::check_wsl()?;

        fn start_or_init(spinner: &mut ProgressBar, after_install: bool) -> Result<StoreImpl> {
            let status = wsl::get_platform_status(consts::CONTAINER_STORE_NAME)?;
            log::trace!("WSL store container status: {status:#?}");

            let store = StoreImpl {};
            match status {
                PlatformStatus::NotInstalled => wsl::import(
                    files::STORE_ROOTFS_NAME,
                    consts::CONTAINER_STORE_NAME,
                    consts::host::DIR_DATA
                        .join_store()
                        .get_or_create()?
                        .to_path_buf(),
                    || {
                        wsl::set_sparse(consts::CONTAINER_STORE_NAME)?;
                        start_or_init(spinner, true)
                    },
                )
                .map_err(|err| {
                    log::error!("Removing leftovers of store files...");
                    let _ = fs::remove_dir_all(consts::host::DIR_CONFIG.join_store());
                    let _ = fs::remove_dir_all(consts::host::DIR_DATA.join_store());
                    err
                }),
                PlatformStatus::Running => {
                    while store
                        .cmd()
                        .run("ps", &[])
                        .output_utf8_ok()?
                        .contains("/sbin/init")
                    {
                        spinner.set_message(
                            "The store is currently initializing. Please wait a moment...",
                        );
                        thread::sleep(Duration::from_millis(500));
                    }
                    if store
                        .cmd()
                        .run("nix", &["store", "ping", "--store", "daemon"])
                        .wait_ok()
                        .is_ok()
                    {
                        Ok(store)
                    } else {
                        let _ = wsl::wsl_command()
                            .arg("--terminate")
                            .arg(consts::CONTAINER_STORE_NAME)
                            .wait_ok()
                            .trace_err("Failed stopping incorrectly started store container");
                        anyhow::bail!(
                            "The store container was started incorrectly. Please try again!"
                        );
                    }
                }
                _ => {
                    if after_install {
                        spinner.set_message(
                            "Initializing store container. This takes a while the first time...",
                        );
                        // build manually, because build in init script deadlocks
                        store
                            .cmd()
                            .run("/sbin/init", &["--until-build"])
                            .output_ok_streaming(|out| log::debug!("{out}\r"))?;
                        store
                            .cmd()
                            .script(format!(
                                r"
unset NIX_REMOTE
nix build $NIX_VERBOSITY --no-link \
    {}#packages.{}.{}.config.build.runtime
",
                                CODCHI_FLAKE_URL, NIX_SYSTEM, NIX_STORE_PACKAGE
                            ))
                            .output_ok_streaming(|out| log::debug!("{out}\r"))?;
                    }

                    store
                        .cmd()
                        .run("/sbin/init", &[])
                        .output_ok_streaming(|out| log::debug!("{out}\r"))?;

                    Ok(store)
                }
            }
        }

        start_or_init(spinner, false)
    }

    fn cmd(&self) -> impl NixDriver {
        LinuxCommandDriver {
            instance_name: consts::CONTAINER_STORE_NAME.to_string(),
        }
    }

    fn _store_path_to_host(
        &self,
        path: &LinuxPath,
        _: Private,
    ) -> anyhow::Result<std::path::PathBuf> {
        self.cmd()
            .run("/bin/wslpath", &["-w", &path.0])
            .output_utf8_ok()
            .map(|path| PathBuf::from(path.trim()))
            .with_context(|| format!("Failed to run 'wslpath' with path '{path}'."))
    }
}

impl MachineDriver for Machine {
    fn cmd(&self) -> impl LinuxCommandTarget {
        LinuxCommandDriver {
            instance_name: machine::machine_name(&self.config.name),
        }
    }

    fn read_platform_status(name: &str, _: Private) -> Result<PlatformStatus> {
        wsl::get_platform_status(&machine::machine_name(name))
    }

    fn install(&self, _: Private) -> Result<()> {
        wsl::import(
            files::MACHINE_ROOTFS_NAME,
            &machine::machine_name(&self.config.name),
            consts::host::DIR_DATA
                .join_machine(&self.config.name)
                .get_or_create()?
                .to_path_buf(),
            || self.start(Private),
        )
    }

    fn start(&self, _: Private) -> Result<()> {
        use consts::machine::INIT_ENV;
        use consts::INIT_EXIT_ERR;
        use consts::INIT_EXIT_SUCCESS;
        use itertools::Itertools;

        let secrets = self
            .config
            .secrets
            .iter()
            .map(|(key, value)| format!(r#"export CODCHI_{key}="{value}""#))
            .join("\n");

        Driver::store()
            .cmd()
            .script(format!(
                r#"
while [ -f "{INIT_ENV}" ]; do
    echo -e '\e[1A\e[KWaiting for machine init env...'
    sleep .25
done
cat <<EOF > "{INIT_ENV}"
CODCHI_DEBUG="{debug}"
CODCHI_MACHINE_NAME="{name}"
{secrets}
EOF
"#,
                debug = *DEBUG,
                name = self.config.name,
            ))
            .output_ok_streaming(|out| log::debug!("{out}\r"))?;

        let log_file = machine::init_log(&self.config.name);
        // let machine_log_prefix = machine_name(&self.name);
        thread::spawn(move || {
            // Tail the init log of the machine until the keyword MACHINE_HAS_STARTED
            Driver::store()
                .cmd()
                .script(format!(
                    r#"
touch "{log_file}"
awk '/^{INIT_EXIT_ERR}$/{{ exit 1}};/^{INIT_EXIT_SUCCESS}$/{{exit 0}};1' < <(tail -f "{log_file}")
"#
                ))
                .output_ok_streaming(|out| log::debug!("{out}\r"))
                .unwrap();
        });
        // .join();

        Ok(())
    }

    fn force_stop(&self, _: Private) -> Result<()> {
        wsl::wsl_command()
            .arg("--terminate")
            .arg(machine_name(&self.config.name))
            .wait_ok()?;
        Ok(())
    }

    fn delete_container(&self, _: Private) -> Result<()> {
        wsl_command()
            .arg("--unregister")
            .arg(machine_name(&self.config.name))
            .wait_ok()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LinuxCommandDriver {
    pub instance_name: String,
}

impl LinuxCommandTarget for LinuxCommandDriver {
    fn build(
        &self,
        user: &Option<LinuxUser>,
        cwd: &Option<LinuxPath>,
        _env: &HashMap<String, String>,
    ) -> std::process::Command {
        let mut cmd = wsl_command();
        cmd.args(["-d", &self.instance_name]);
        cmd.args(["--cd", &cwd.clone().map(|p| p.0).unwrap_or("/".to_string())]);

        // https://devblogs.microsoft.com/commandline/share-environment-vars-between-wsl-and-windows/
        cmd.env("CODCHI_DEBUG", if *DEBUG { "1" } else { "" });
        cmd.env("CODCHI_MACHINE_NAME", &self.instance_name); // only neccessary for machines, ignored in store
        cmd.env("CODCHI_IS_STORE", "1"); // only neccessary for store, ignored in machines
        cmd.env(
            "WSL_CODCHI_DIR_CONFIG",
            consts::host::DIR_CONFIG.as_os_str(),
        );
        cmd.env("WSL_CODCHI_DIR_DATA", consts::host::DIR_DATA.as_os_str());
        let mut wslenv = env::var_os("WSLENV").unwrap_or("".into());
        // log::trace!("WSLENV: {wslenv:?}");
        if !wslenv.is_empty() {
            wslenv.push(":");
        }
        wslenv.push(
            "CODCHI_DEBUG:CODCHI_MACHINE_NAME:CODCHI_IS_STORE:WSL_CODCHI_DIR_CONFIG/up:WSL_CODCHI_DIR_DATA/up",
        );
        if CodchiConfig::get().vcxsrv.enable {
            cmd.env("CODCHI_WSL_USE_VCXSRV", "1");
            wslenv.push(":CODCHI_WSL_USE_VCXSRV");
        }
        cmd.env("WSLENV", wslenv);

        match &user {
            Some(LinuxUser::Root) => {
                cmd.args(["--user", "root"]);
            }
            Some(LinuxUser::Default) => {
                cmd.args(["--user", consts::user::DEFAULT_NAME]);
            }
            None => {}
        };
        cmd.arg("--");
        cmd
    }

    fn get_driver(&self) -> LinuxCommandDriver {
        self.clone()
    }

    fn quote_shell_arg(&self, arg: &str) -> String {
        format!("'{}'", arg)
    }
}
