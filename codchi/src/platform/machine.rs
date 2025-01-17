use super::{
    platform::HostImpl, Host, LinuxCommandBuilder, LinuxCommandTarget, LinuxUser, NixDriver,
};
use crate::{
    cli::CODCHI_DRIVER_MODULE, config::{EnvSecret, FlakeLocation, MachineConfig}, consts::{self, host, ToPath}, logging::{hide_progress, log_progress, set_progress_status, with_suspended_progress}, platform::{self, CommandExt, Driver, Store}, progress_scope, util::PathExt
};
use anyhow::{bail, Context, Result};
use itertools::Itertools;
use log::Level;
use std::{collections::HashMap, fs, sync::mpsc::channel, thread};

pub trait MachineDriver: Sized {
    fn cmd(&self) -> impl LinuxCommandTarget;

    /// Read if container is running / stopped / not installed
    fn read_platform_status(name: &str) -> Result<PlatformStatus>;

    /// Import and configure machine container
    fn install(&self) -> Result<()>;

    /// Start container
    fn start(&self) -> Result<()>;

    /// Stop / Kill container
    fn stop(&self, force: bool) -> Result<()>;

    /// Delete container
    fn delete_container(&self) -> Result<()>;

    fn create_exec_cmd(&self, cmd: &[&str]) -> LinuxCommandBuilder;

    /// Export file system of a machine to a tar WITHOUT starting the store or the machine.
    fn tar(&self, target_file: &std::path::Path) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub config: MachineConfig,
    pub config_status: ConfigStatus,
    pub platform_status: PlatformStatus,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PlatformStatus {
    NotInstalled,
    Stopped,
    Running,
}

/// The (NixOS) status of the machine configuration
#[derive(Debug, PartialEq, Eq, Clone, strum::EnumString, strum::Display)]
pub enum ConfigStatus {
    /// Machine was added / configured, but not built and installed
    NotInstalled,

    /// Machine was already built and installed but config has changed (flake.nix has changed)
    Modified,

    /// Machine was already built and installed but updates are available (flake.lock has changed)
    UpdatesAvailable,

    /// Machine is built, installed and up to date
    UpToDate,
}

impl Machine {
    pub fn update_status(mut self) -> Result<Self> {
        self.platform_status = Self::read_platform_status(&self.config.name)?;
        self.config_status = {
            use ConfigStatus::*;
            let machine_dir = consts::host::DIR_CONFIG.join_machine(&self.config.name);
            if self.platform_status == PlatformStatus::NotInstalled
                || fs::symlink_metadata(machine_dir.join("system")).is_err()
            {
                NotInstalled
            } else {
                Driver::store()
                    .cmd()
                    .script(format!(
                        /* bash */
                        r#"
set -x
if [ -n "$(git diff flake.nix)" ]; then
  printf "{Modified}"
elif [ -n "$(git diff flake.lock)" ]; then
  printf "{UpdatesAvailable}"
else
  printf "{UpToDate}"
fi
"#,
                    ))
                    .with_cwd(consts::store::DIR_CONFIG.join_machine(&self.config.name))
                    .output_from_str()?
            }
        };
        Ok(self)
    }
    pub fn read(config: MachineConfig, update_status: bool) -> Result<Self> {
        let machine = Self {
            config,
            config_status: ConfigStatus::NotInstalled,
            platform_status: PlatformStatus::NotInstalled,
        };
        if update_status {
            machine.update_status()
        } else {
            Ok(machine)
        }
    }

    /// Returns Err if machine doesn't exist
    pub fn by_name(name: &str, update_status: bool) -> Result<Self> {
        let (_, cfg) = MachineConfig::open_existing(name, false)?;
        Self::read(cfg, update_status)
    }

    pub fn list(update_status: bool) -> Result<Vec<Self>> {
        MachineConfig::list()?
            .into_iter()
            .map(|cfg| Self::read(cfg, update_status))
            .collect()
    }

    pub fn write_flake(&self) -> Result<()> {
        let machine_dir = host::DIR_CONFIG.join_machine(&self.config.name);
        machine_dir.get_or_create()?;

        let flake = {
            let codchi_url = consts::CODCHI_FLAKE_URL;
            let codchi_driver = CODCHI_DRIVER_MODULE;
            let module_inputs = self
                .config
                .modules
                .iter()
                .map(|(name, url)| {
                    format!(
                        r#"    "{name}".url = "{}";"#,
                        url.to_nix_url(&self.config.name)
                    )
                })
                .join("\n");
            let driver = platform::NIXOS_DRIVER_NAME;
            let nix_system = consts::NIX_SYSTEM;
            let nixpkgs = if let Some(name) = &self.config.nixpkgs_from {
                format!(r#"inputs."{name}".inputs.nixpkgs"#)
            } else {
                "inputs.codchi_driver.inputs.nixpkgs".to_string()
            };
            let modules = self
                .config
                .modules
                .iter()
                .map(|(name, url)| {
                    format!(
                        r#"        inputs."{name}".{module_name}"#,
                        module_name = url.flake_attr
                    )
                })
                .join("\n");
            format!(
                r#"{{
  inputs = {{
    {codchi_driver}.url = "{codchi_url}";
{module_inputs}
  }};
  outputs = inputs: {{
    nixosConfigurations.default = inputs.{codchi_driver}.lib.codeMachine {{
      driver = "{driver}";
      system = "{nix_system}";
      nixpkgs = {nixpkgs};
      modules = [
{modules}
      ];
    }};
  }};
}}"#
            )
        };
        fs::write(machine_dir.join("flake.nix"), flake)?;

        // let _ = Progress::new().with_status("Initializing machine...");
        Driver::store()
            .cmd()
            .script(
                r#"
if [ ! -d .git ]; then
  git init -q
  git add flake.nix
fi
"#
                .to_string(),
            )
            .with_cwd(consts::store::DIR_CONFIG.join_machine(&self.config.name))
            .wait_ok()?;

        Ok(())
    }

    pub fn update_flake(&self) -> Result<()> {
        Driver::store()
            .cmd()
            .script(r#"ndd $NIX_VERBOSITY flake update"#.to_string())
            .with_cwd(consts::store::DIR_CONFIG.join_machine(&self.config.name))
            .output_ok_streaming(channel().1, |line| {
                log_progress("build", log::Level::Debug, &line)
            })?;
        Ok(())
    }

    pub fn build(&self, no_update: bool) -> Result<()> {
        self.write_flake()?;

        set_progress_status(format!("Building {}...", self.config.name));
        let has_local = self
            .config
            .modules
            .iter()
            .any(|(_, flake)| matches!(flake.location, FlakeLocation::Local { .. }));
        let awaker = if has_local {
            match Self::read_platform_status(&self.config.name)? {
                PlatformStatus::Stopped => {
                    set_progress_status(format!("Starting {}...", self.config.name));
                    self.start()?;
                }
                PlatformStatus::Running => {}
                PlatformStatus::NotInstalled => bail!(
                    "Can't build machine {} as it uses local modules but wasn't installed yet!",
                    self.config.name
                ),
            }

            let mut cmd = self.cmd().run("sleep", &["infinity"]);
            Some(thread::spawn(move || {
                log::trace!("Keeping machine awake: {:?}", cmd.wait_ok());
            }))
        } else {
            None
        };

        set_progress_status(format!("Building {}...", self.config.name));
        if !no_update {
            self.update_flake()?;
        }
        Driver::store()
            .cmd()
            .script(
                r#"
NIX_CFG_FILE="$(ndd build $NIX_VERBOSITY --no-link --print-out-paths \
    '.#nixosConfigurations.default.config.environment.etc."nix/nix.conf".source')"
export NIX_CONFIG="$(cat $NIX_CFG_FILE)"
if [ ! -e system ]; then
  ndd $NIX_VERBOSITY profile install --option warn-dirty false --profile system \
        '.#nixosConfigurations.default.config.system.build.toplevel'
else
  ndd $NIX_VERBOSITY profile upgrade --option warn-dirty false --profile system '.*'
fi
pwd
git add flake.*
"#
                .to_string(),
            )
            .with_cwd(consts::store::DIR_CONFIG.join_machine(&self.config.name))
            .output_ok_streaming(channel().1, |line| {
                log_progress("build", log::Level::Debug, &line)
            })?;

        if awaker.is_some() {
            log::trace!(
                "Killing awaker: {:?}",
                self.cmd().run("pkill", &["sleep"]).wait_ok()
            );
        }
        let secrets: HashMap<String, EnvSecret> = Driver::store().cmd().eval(
            consts::store::DIR_CONFIG.join_machine(&self.config.name),
            "nixosConfigurations.default.config.codchi.secrets.env",
        )?;

        set_progress_status("Evaluating secrets...");
        let (lock, mut cfg) = MachineConfig::open_existing(&self.config.name, true)?;
        let old_secrets = &cfg.secrets;
        let mut all_secrets = HashMap::new();
        for secret in secrets.values() {
            match old_secrets.get(&secret.name) {
                Some(existing) => {
                    log::debug!("Keeping existing secret {}.", secret.name);
                    all_secrets.insert(secret.name.clone(), existing.clone());
                }
                None => {
                    let value = with_suspended_progress(|| {
                        inquire::Password::new(&format!(
                            "Please enter secret '{}' (Toggle input mask with <Ctrl+R>):",
                            secret.name
                        ))
                        .without_confirmation()
                        .with_display_mode(inquire::PasswordDisplayMode::Masked)
                        .with_help_message(secret.description.trim())
                        .with_display_toggle_enabled()
                        .prompt()
                    })?;
                    all_secrets.insert(secret.name.clone(), value);
                }
            }
        }
        cfg.secrets = all_secrets;
        cfg.write(lock)?;

        set_progress_status(format!("Building {}...", self.config.name));
        let status = Self::read_platform_status(&self.config.name)?;
        if status == PlatformStatus::NotInstalled {
            set_progress_status(format!("Installing {}...", self.config.name));
            self.install().inspect_err(|_err| {
                if !log::log_enabled!(Level::Debug) {
                    log::error!(
                        "Removing leftovers of machine files for {}...",
                        self.config.name
                    );
                    log::trace!(
                        "Deleting config data for {}: {:?}",
                        self.config.name,
                        fs::remove_dir_all(host::DIR_CONFIG.join_machine(&self.config.name))
                    );
                    log::trace!(
                        "Deleting data for {}: {:?}",
                        self.config.name,
                        fs::remove_dir_all(host::DIR_DATA.join_machine(&self.config.name))
                    );
                }
            })?;
        } else {
            if status == PlatformStatus::Stopped {
                set_progress_status(format!("Starting {}...", self.config.name));
                self.start()?;
            }
            self.cmd()
                .run(
                    "/nix/var/nix/profiles/system/bin/switch-to-configuration",
                    &["switch"],
                )
                .with_user(LinuxUser::Root)
                .wait_ok()?;
        }

        set_progress_status("Updating start menu shortcuts...");
        HostImpl::write_machine_shortcuts(self)?;
        HostImpl::post_install(&self.config.name)?;

        hide_progress();

        Ok(())
    }

    pub fn delete(self, im_really_sure: bool) -> Result<()> {
        let name = &self.config.name;
        if !im_really_sure
            && !inquire::Confirm::new(&format!("Delete '{name}'?",))
                .with_help_message(&format!(
                    "This will remove all files associated with '{name}'"
                ))
                .prompt()?
        {
            bail!("Canceled deletion.");
        }

        set_progress_status(format!("Stopping {name}"));
        if self.platform_status == PlatformStatus::Running {
            self.stop(true)?;
        }
        set_progress_status(format!("Deleting container of {name}"));
        if self.platform_status != PlatformStatus::NotInstalled {
            MachineDriver::delete_container(&self)?;
        }

        set_progress_status(format!("Deleting files from {name}"));
        Driver::store()
            .cmd()
            .run(
                "rm",
                &[
                    "-rf",
                    &consts::store::DIR_DATA.join_machine(&self.config.name).0,
                    &consts::store::DIR_CONFIG.join_machine(&self.config.name).0,
                ],
            )
            .wait_ok()
            .context("Failed deleting data.")?;

        log::trace!(
            "Deleting config data for {}: {:?}",
            self.config.name,
            fs::remove_dir_all(host::DIR_CONFIG.join_machine(&self.config.name))
        );
        log::trace!(
            "Deleting data for {}: {:?}",
            self.config.name,
            fs::remove_dir_all(host::DIR_DATA.join_machine(&self.config.name))
        );

        set_progress_status("Deleting start menu shortcuts...");
        HostImpl::delete_shortcuts(&self.config.name)?;
        HostImpl::post_delete(&self.config.name)?;

        println!("Successfully deleted {}. You might also want to run a garbage collection (`codchi gc`).", self.config.name);

        Ok(())
    }

    pub fn exec(&self, cmd: &[String]) -> Result<()> {
        if self.config_status == ConfigStatus::NotInstalled
            || self.platform_status == PlatformStatus::NotInstalled
        {
            bail!(
                "Machine {} wasn't installed yet. Install with `codchi rebuild {}`.",
                self.config.name,
                self.config.name
            );
        }

        #[cfg(target_os = "windows")]
        Driver::host().start_vcxsrv(false)?;

        set_progress_status(format!("Starting {}...", self.config.name));
        self.start()?;
        hide_progress();
        self.create_exec_cmd(&cmd.iter().map(|str| str.as_str()).collect_vec())
            .exec()?;
        Ok(())
    }

    pub fn run_init_script(&self) -> Result<()> {
        progress_scope! {
            set_progress_status(format!("Running init script of machine '{}'...", self.config.name));
            self.cmd().run("codchi-init", &[]).wait_ok()?;
        }

        Ok(())
    }
}
