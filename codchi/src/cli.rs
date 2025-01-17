pub use self::module::*;

use clap::builder::*;
use clap::*;
use clap_verbosity_flag::{InfoLevel, LogLevel, Verbosity};
use git_url_parse::{GitUrl, Scheme};
use lazy_regex::regex_is_match;
use log::Level;
use std::{
    path::PathBuf,
    str::FromStr,
    sync::{LazyLock, OnceLock},
};

#[allow(dead_code)]
pub static CLI_ARGS: OnceLock<Cli> = OnceLock::new();
#[allow(dead_code)]
pub static DEBUG: LazyLock<bool> = LazyLock::new(|| {
    CLI_ARGS
        .get()
        .and_then(|cli| cli.verbose.log_level())
        .or(<DefaultLogLevel as LogLevel>::default())
        .unwrap()
        >= Level::Debug
});

type DefaultLogLevel = InfoLevel;

#[derive(Debug, Parser, Clone)]
#[command(
    author,
    version,
    long_version = format!("v{}\n{}",
        option_env!("CARGO_PKG_VERSION").unwrap_or(""),
        option_env!("CODCHI_GIT_COMMIT").unwrap_or(""),
    ),
    about = "Codchi is a tool that manages your project's development environment in a reproducible
    and easy-to-use way. Setting up a development environment should be as easy as a `git clone`!

- [What is Codchi?](https://codchi.dev/introduction/what-is-codchi)
- [Configuring Codchi](https://codchi.dev/introduction/config)
- [Codchi Module Configuration](https://codchi.dev/config/overview)

The default subcommand is `codchi status`.", 
    after_long_help = r#"
# EXAMPLES

Create a code machine with a Java devenv and launch the IntelliJ GUI:
```
codchi init java-demo https://github.com/aformatik/codchi nixosModules.jvm
codchi exec java-demo idea-community
```

Create an empty code machine (e.g. to use `flake.nix` or `devenv.sh`):
```
codchi init devenv-sh
codchi exec devenv-sh
nix run nixpkgs#nushell
```

Remove the code machines above and collect their garbage:
```
codchi delete java-demo
> Delete 'java-demo'? Yes
codchi delete devenv-sh
> Delete 'devenv-sh'? Yes
codchi gc
```
"#,
    disable_help_subcommand = true,
    // infer_subcommands = true,
    // disable_help_flag = true,
)]
pub struct Cli {
    #[command(flatten)]
    pub verbose: Verbosity<DefaultLogLevel>,

    /// Produce output in JSON format, suitable for consumption by another program.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Subcommand, Clone)]
pub enum Cmd {
    /// List installed code machines and their status.
    Status {},

    #[clap(
        about = "Initialize a new code machine",
        long_about = r#"
A code machine usually has one module that resides in the repository of the project you want to
develop. To create a machine with this module, use the following command. Only the url to the
repository is mandatory. Codchi will try to guess the rest of the information and prompt you if
neccessary."#,
        after_long_help = r#"
### Which nixpkgs should I use?
Since every code machine is a NixOS system, it needs a version of nixpkgs (the
collection containing all Nix programs). There are two options:

1. Use Codchi's nixpkgs: Every release of codchi has a pinned version of nixpkgs which
   is consistent across all code machines on a given host system. Because the Nix store
   is shared among all machines, this results in fewer package downloads, saves disk
   space and is faster during installation or updates. The downside is that code
   machines aren't perfectly reproducibile anymore, since the nixpkgs version can
   change with every Codchi version. In reality this shouldn't be a big deal because
   Codchi adheres to NixOS's release schedule (every 6 months) and keeps nixpkgs
   consistent across this timespan.
2. Use the module's nixpkgs: If a module of a code machine has a nixpkgs input inside its
   `flake.nix`, the code machine can use (Nix language: "follow") it.
    - Pro: Exact reproducibility among machines with the same module and nixpkgs.
    - Con: More packages to download, more disk space and slower during installation and updates.

# EXAMPLES

Create a machine with a Java devenv:
```
codchi init <MACHINE_NAME> https://github.com/aformatik/codchi nixosModules.jvm
```
Create a machine with Codchi's devenv (rust):
```
codchi init <MACHINE_NAME> https://github.com/aformatik/codchi nixosModules.codchi
```
Let Codchi search for available modules in `<REPO_URL>` and prompt you with it:
```
codchi init <MACHINE_NAME> <REPO_URL>
```
Create an empty base machine:
```
codchi init <MACHINE_NAME>
```"#
    )]
    Init {
        /// Name of the code machine.
        machine_name: String,

        /// HTTP(S) URL to the git repository which holds the Codchi module.
        ///
        /// When omitted Codchi will create a base machine without modules.
        url: Option<CodchiUrl>,

        #[command(flatten)]
        input_options: Box<InputOptions>,

        /// A list of flake paths to the Codchi modules you whish to add. Currently supported:
        /// 'codchiModules.<module>' or 'nixosModules.<module>'. If not sure which modules are
        /// available, leave this empty. Codchi will prompt you with a list of possible options.
        ///
        /// With `--dont-prompt` this argument is required.
        #[arg(requires = "url")]
        module_paths: Vec<ModuleAttrPath>,
    },

    #[clap(
        about = "Clone a project and initialize its code machine using the cloned module.",
        long_about = r#"
This command unifies the following commands:
```
codchi init <MACHINE_NAME> <URL>
codchi exec <MACHINE_NAME> git clone <URL> <TARGET_DIR>
codchi module set <MACHINE_NAME> <MODULE_NAME> --url <TARGET_DIR>
```
See the documentation of each command for more information.
"#,
        after_long_help = r#"
# EXAMPLES

Install and clone the Codchi repo / machine:
```
codchi clone <MACHINE_NAME> https://github.com/aformatik/codchi nixosModules.codchi
codchi exec <MACHINE_NAME>
cd codchi/codchi
cargo build
```
"#
    )]
    Clone {
        /// Name of the code machine.
        machine_name: String,

        /// Same as `git clone --depth`
        #[arg(long)]
        depth: Option<u32>,

        /// Same as `git clone --single-branch`
        #[arg(long)]
        single_branch: bool,

        /// Same as `git clone --recurse-submodules`
        #[arg(long)]
        recurse_submodules: bool,

        /// Same as `git clone --shallow-submodules`
        #[arg(long)]
        shallow_submodules: bool,

        /// The target directory, relative to `$HOME`, for the git repo. If left empty this will
        /// chosen by git.
        #[arg(long, short = 'd')]
        dir: Option<RelativePath>,

        /// Don't switch to the local codchi module
        #[arg(long, short = 'r')]
        keep_remote: bool,

        /// HTTP(S) URL to the git repository.
        url: CodchiUrl,

        #[command(flatten)]
        input_options: Box<InputOptions>,

        /// A list of flake paths to the Codchi modules you whish to add. Currently supported:
        /// 'codchiModules.<module>' or 'nixosModules.<module>'. If not sure which modules are
        /// available, leave this empty. Codchi will prompt you with a list of possible options.
        ///
        /// With `--dont-prompt` this argument is required.
        #[arg(requires = "url")]
        module_paths: Vec<ModuleAttrPath>,
    },

    #[command(subcommand)]
    #[clap(
        aliases = &["mod"],
        about = "Manage modules of code machines.",
        long_about = r#"
Each code machine has a list of modules (often just one) which are identified by a name. There can
also be machines without modules (a base machine). You can add, modify, remove a module or switch a
module between local and remote configuration.
        "#,
        after_long_help = r#"
### Local Configuration

While working on a code machine module, any change to the code must first be pushed online before
Codchi can pull and apply the change. This can quickly become annoying, especially if you're
actively developing the module or experimenting a lot. Luckily, a machine module can be switched to
local repository inside the code machine.

It's good practise to have the `.nix` configuration files in the repository of your project itself.
This way Codchi can spin up a machine for every commit of your project. Also you'll have the
project checked out in your code machine anyway, so step 1 shouldn't be neccessary.

1. First of all, clone the repository containing the module configuration inside <MACHINE_NAME>.
   For Codchi to be able to find it, the repository has reside in a folder inside the machine's
   `$HOME`, which is `/home/codchi`.
```
git clone https://github.com/my/repo ~/my-project-name
```

2. Upon creation of <MACHINE_NAME>, if you didn't explicitly specify a module name for your
   repository, Codchi guesses a name from its url. List <MACHINE_NAME>'s modules to get the actual
   module name:
```bash
codchi module list <MACHINE_NAME>
┌─────────────┬────────────────────┬────────────────────────┐
│ Name        ┆ Url                ┆ Flake Module           │
╞═════════════╪════════════════════╪════════════════════════╡
│ repo-module ┆ github.com/my/repo ┆ nixosModules.my-module │
└─────────────┴────────────────────┴────────────────────────┘
```
3. Now you can tell Codchi to switch the module pointing to the remote repository to your local
   copy. Note that that 'my-project-name' is the path inside <MACHINE_NAME> relative to $HOME:
```bash
codchi module set <MACHINE_NAME> <MODULE_NAME> --url=my-project-name
```

If you don't already have a module (e.g. after creating a base machine), you can just add a module
pointing to a local repository. The name of the new module will be inferred by Codchi:
```bash
codchi module add <MACHINE_NAME> my-project-name
```
"#
    )]
    Module(ModuleCmd),

    #[clap(
        about = "Apply changes to a code machine. \
By default this also fetches updates for each module.",
        long_about = r#"
When a code machine is modified, it needs to be rebuilt in order to apply the changes.
Modifications include:

- A module was added, modified or removed
- The configuration of a remote module was modified
- The configuration of a local module was modified

To prevent the fetching of updates, use `--no-update`.
"#,
        after_long_help = r#"
# EXAMPLES

Apply local and remote changes:
```
codchi rebuild <MACHINE_NAME>
```
Apply only local changes:
```
codchi rebuild <MACHINE_NAME> --no-update
```
"#
    )]
    Rebuild {
        /// Don't fetch module updates
        #[arg(long, short = 'n')]
        no_update: bool,

        /// Name of the code machine
        name: String,
    },

    #[clap(
        aliases = &["run"],
        about = "Execute a command inside a code machine. \
        The code machine will be started if not already running.",
        after_long_help = r#"
### Running graphical programs

Programs which provide shortcuts (usually graphical programs) are added to your host's system menu
as shortcuts and can be started by clicking them.

# EXAMPLES

Open a shell inside <MACHINE_NAME>:
```
codchi exec <MACHINE_NAME>
```
Run `PROGRAM [ARGS...]` inside <MACHINE_NAME>:
```
codchi exec <MACHINE_NAME> PROGRAM [ARGS...]
```
For example, `uname -a`:
```
codchi exec <MACHINE_NAME> uname -a
```
If some options conflict with Codchi's options, you can escape them with `--`:
```
codchi exec <MACHINE_NAME> -- PROGRAM [ARGS...]
```
"#
    )]
    Exec {
        /// Name of the code machine
        name: String,

        /// Command with arguments to run. Leave empty to open a shell.
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },

    #[clap(
        about = "Delete a code machine with all associated files.",
        long_about = r#"
### WARNING: This will irrevocably delete all files belonging to the code machine!
"#
    )]
    Delete {
        /// Don't prompt for confirmation and delete this machine immediately.
        #[arg(long)]
        i_am_really_sure: bool,

        /// Name of the code machine
        name: String,
    },

    #[clap(
        group(ArgGroup::new("exclusive").args(&["all", "machines"])),
        about = "Perform garbage collection of old nix store paths.",
        long_about = r#"
All programs and system files reside in the append-only Nix Store. Over time the store will grow in
size noticably, because everytime a `codchi rebuild` is run, new files get added to the store.
Therefore you might want to perform a garbage collection from time to time.

By default, garbage collection will **not** delete old machine generations in order to allow
instantaneous rollbacks. The drawback is that the store paths refered to in the old generations
never get freed. To also delete old generations, use `--delete-old` with an optional minimum age
(in days). Note that only explicitly listed machines (`MACHINES...`) will be processed. To
process all installed machines, use `--all`.

### WARNING
In Codchi, the Nix Store is shared across all code machines, which means that the garbage collector
also shares a directory of roots ("gcroots") that should be preserved, such as the current system
configuration of each code machine. On a vanilla NixOS system, local gcroots (such as those created
by `nix build` or direnv) are automatically registered and protected from garbage collection.
However, this doesn't work in Codchi at the moment, because the garbage collector has a different
view on the file system of each code machine. As a result, the local gcroots of individual machines
become invalid. For example, a gcroot pointing to `/home/codchi/result` on machine `foo` is invalid
from the perspective of Codchi, as the correct path would be `/data/machine/foo/result`.

This isn't a huge issue, as the deleted store paths can be redownloaded or rebuilt.
"#,
        after_long_help = r#"
### Large WSL Distributions

On Windows, the store is inside the WSL distribution 'codchistore'. By default WSL distributions
only grow in size, once used disk space is not automatically reclaimed. Codchi will try to set the
distribution to sparse mode which should automatically free unused space. If this doesn't work, you
can do it automatically with
```bash
wsl.exe --manage codchistore --set-sparse true
```
Beware that WSL needs to be shut down for this which will close all running Linux programs.

If sparse mode somehow doesn't work correctly, you can manually shrink a WSL distribution like this:
```ps1
wsl.exe --terminate codchistore
diskpart # this will need admin rights
select vdisk file="C:\Users\YOUR_USER\AppData\Local\codchi\store\ext4.vhdx"
compact vdisk
exit
```

# EXAMPLES

Delete generations older than one month of <MACHINE_1> and <MACHINE_2>:
```
codchi gc --delete-old 30 <MACHINE_1> <MACHINE_2>
```
Delete all but the current generation from each installed machine:
```
codchi gc --all --delete-old
```
"#
    )]
    GC {
        /// Delete old machine generations. AGE is the minimum age in days to be deleted [default: 0]
        #[arg(long, short = 'd', value_name = "AGE")]
        delete_old: Option<Option<u16>>,

        /// Process all machines. Requires `--delete-old`.
        #[arg(long, short = 'a', requires = "delete_old")]
        all: bool,

        /// Machines to be processed. Requires `--delete-old`.
        #[arg(requires = "delete_old")]
        machines: Vec<String>,
    },

    #[clap(
        about = "Generate shell completions.",
        long_about = r#"
See the following docs on how to register the completions with your shell:

- [Windows](https://codchi.dev/introduction/installation#shell-completions)
- [Linux](https://codchi.dev/introduction/installation#shell-completions-1)
- [NixOS](https://codchi.dev/introduction/installation#shell-completions-2)
"#
    )]
    Completion {
        /// The shell to generate the completions for.
        #[arg(value_enum)]
        shell: clap_complete_command::Shell,
    },
    ///
    /// Start the codchi tray if not running.
    #[clap(hide = true)]
    Tray {},

    #[clap(about = "Export the file system of a code machine.")]
    Tar {
        /// Name of the code machine
        name: String,

        /// Path to export to.
        target_file: PathBuf,
    },

    #[command(subcommand)]
    #[clap(about = "Utilities for interacting with the `codchistore` container.")]
    Store(StoreCmd),
}

mod module {
    use super::*;
    use core::fmt;
    use lazy_regex::regex_captures;
    use std::fmt::Display;

    pub static CODCHI_DRIVER_MODULE: &str = "codchi_driver";

    #[derive(Debug, Subcommand, Clone)]
    pub enum ModuleCmd {
        /// Lists modules of a code machine
        #[clap(aliases = &["ls"])]
        List {
            /// Name of the code machine
            name: String,
        },

        #[clap(
            about = "Add a module to a code machine.",
            after_long_help = r#"
# EXAMPLES

### Personal modules

Sometimes you might want to customize a code machine, but don't want to commit it to upstream to
the main module of the code machine which is also used by other developers. For example, you might
want to use a different editor than the rest of the project's contributors. Fortunately, you can
add as many modules to a machine as you like! This is especially handy for sharing your personal
configuration between different code machines:
```
codchi module add <MACHINE_NAME> https://github.com/my/cool/nixconfig nixosModules.vim
```

### Local modules
See the **EXTRA** section of `codchi module` for more information.
Given you already have a git repository containing a code machine module checked out inside
<MACHINE_NAME> at the path `$HOME/my-additional-module`. You can add it to the code machine via:
```bash
codchi module add <MACHINE_NAME> my-additional-module <MODULE_PATHS...>
```
**Note that the <URL> (`my-additional-module`) is relative to `$HOME` inside <MACHINE_NAME> and has
no leading `~` or `./`.** If you leave <MODULE_PATHS...> empty, Codchi will prompt you with a list
of available modules, just like with `codchi init`.
"#
        )]
        Add {
            /// Name of the code machine
            machine_name: String,

            /// HTTP(S) URL or file path to the codchi module
            url: CodchiUrl,

            #[command(flatten)]
            options: Box<InputOptions>,

            /// A list of flake paths to the NixOS modules you whish to add.
            /// Currently supported: 'codchiModules.<module>' or 'nixosModules.<module>'
            module_paths: Vec<ModuleAttrPath>,
        },

        #[clap(
            about = "Modify a module of a code machine.",
            long_about = r#"
Change the upstream url, switch the git branch, rename the module, edit access information, switch
between remote and local repositories or set which nixpkgs to use.
            "#,
            after_long_help = r#"
# EXAMPLES

Change the branch of <MODULE_NAME>:
```
codchi module set <MACHINE_NAME> <MODULE_NAME> --branch develop
```
Rename <MODULE_NAME> to <NEW_NAME>
```
codchi module set <MACHINE_NAME> <MODULE_NAME> --new-name <NEW_NAME>
```
Switch the upstream URL:
```
codchi module set <MACHINE_NAME> <MODULE_NAME> --url https://github.com/my/new/repo
```

### Switch to a module inside `<CODE_MACHINE>`
See the **EXTRA** section of `codchi module` for more information. **Note that the <URL>
(`my-project-name`) points to a git repository relative to `$HOME` inside <MACHINE_NAME> and has no
leading `~` or `./`.**
```bash
codchi module set <MACHINE_NAME> <MODULE_NAME> --url=my-project-name
```
            "#
        )]
        Set {
            /// Name of the code machine
            machine_name: String,

            /// The name of the module to modify
            name: ModuleName,

            /// HTTP(S) URL or file path to the repository of the module
            #[arg(long, short = 'u')]
            url: Option<CodchiUrl>,

            #[command(flatten)]
            options: Box<InputOptions>,

            /// The new name of the module => TODO mod rename command
            #[arg(long, short = 'n')]
            new_name: Option<ModuleName>,

            /// The flake path to the NixOS module you whish to use.
            /// Currently supported: 'codchiModules.<module>' or 'nixosModules.<module>'
            #[arg(long, short = 'm')]
            module_path: Option<ModuleAttrPath>,
        },

        /// Delete a module of a code machine.
        ///
        /// This will **NOT** delete user data, even if its the last remaining module! It will only
        /// remove programs, services and other configuration defined in that module.
        #[clap(aliases = &["rm", "remove"])]
        Delete {
            /// Name of the code machine
            name: String,

            /// Name of the module (You can list them with `codchi module ls NAME`)
            module_name: ModuleName,
        },
    }

    #[derive(clap::Args, Debug, Clone, Default, Eq, PartialEq)]
    // branch an tag are mutually exclusive
    #[clap(
        group(ArgGroup::new("exclusive").args(&["branch", "tag"])),
    )]
    pub struct InputOptions {
        /// Don't prompt for confirmation and accept all of Codchi's defaults. Usefull for
        /// using Codchi in scripts.
        #[arg(long, short = 'y')]
        pub dont_prompt: bool,

        /// Don't automatically build this machine after this command completes.
        #[arg(long, short = 'N')]
        pub no_build: bool,

        /// Whether to use Nixpkgs from the **local** Codchi installation or the **remote** URL.
        /// See the "Which nixpkgs should I use?" section of `codchi init` for more details.
        ///
        /// By default Codchi will use **remote** if <URL> defines a nixpkgs input in its flake.
        /// Otherwise it will use the **local** nixpkgs.
        #[arg(long, short = 'p')]
        pub use_nixpkgs: Option<NixpkgsLocation>,

        /// Authorisation string for private repositories. Generally, the syntax is `<user>:<token>`
        /// or just `<token>`.
        ///
        /// GitHub for example has something like `ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`
        /// whereas GitLab uses `oauth2:glpat-xxxxxxxxxxxxxxxxxxxx`.
        #[arg(long, short)]
        pub auth: Option<String>,

        /// The git branch to use for the code machine module.
        #[arg(long, short)]
        pub branch: Option<String>,

        /// The git tag to use for the code machine module.
        #[arg(long)]
        pub tag: Option<String>,

        /// The git commit to use for the code machine module.
        #[arg(long, short)]
        pub commit: Option<String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ModuleAttrPath {
        /// codchiModules or nixosModules
        pub base: String,
        pub module: String,
    }

    impl Display for ModuleAttrPath {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}.{}", self.base, self.module)
        }
    }

    impl FromStr for ModuleAttrPath {
        type Err = String;

        fn from_str(string: &str) -> Result<Self, Self::Err> {
            match regex_captures!(
                r"^(nixosModules|codchiModules)\.([a-zA-Z\_][a-zA-Z0-9\_\'\-]*)$",
                string
            ) {
                Some((_, base, module)) => Ok(ModuleAttrPath {
                    base: base.to_string(),
                    module: module.to_string(),
                }),
                None => Err("Please refer to a NixOS module as 'codchiModules.<name>' \
                    or 'nixosModules.<name>'"
                    .to_string()),
            }
        }
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub struct ModuleName(pub String);

    impl Display for ModuleName {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl FromStr for ModuleName {
        type Err = String;

        fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
            if s != CODCHI_DRIVER_MODULE {
                Ok(ModuleName(s.to_string()))
            } else {
                Err("The name 'codchi_driver' is reserved.".to_string())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, ValueEnum)]
pub enum NixpkgsLocation {
    Local,
    Remote,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CodchiUrl {
    pub git_url: GitUrl,
    pub original: String,
}

impl FromStr for CodchiUrl {
    type Err = <GitUrl as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let git_url = GitUrl::from_str(s)?;

        Ok(Self {
            git_url,
            original: s.to_owned(),
        })
    }
}

impl From<&CodchiUrl> for GitUrl {
    fn from(val: &CodchiUrl) -> Self {
        match val.git_url.scheme {
            Scheme::File => {
                let mut url = val.git_url.clone();
                url.path = val.original.clone();
                url
            }
            _ => val.git_url.clone(),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RelativePath(pub String);

impl FromStr for RelativePath {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !regex_is_match!("[a-zA-Z0-9-_/ :]", s) {
            return Err(
                r#"Only paths matching the pattern "[a-zA-Z0-9-_/ :]" are allowed."#.to_string(),
            );
        }
        Ok(Self(s.to_string()))
    }
}

#[derive(Debug, Subcommand, Clone)]
pub enum StoreCmd {
    #[clap(about = "Open a debug shell inside `codchistore` without starting \
/ requiring any services. Usefull for debugging.")]
    Debug,

    #[cfg(target_os = "windows")]
    #[clap(about = "Try to restore the `codchistore` container without deleting /nix/store.")]
    Recover,
}
