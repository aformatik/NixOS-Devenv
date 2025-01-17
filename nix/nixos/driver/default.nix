{ inputs, lib, pkgs, config, consts, ... }:
let
  inherit (lib) mkOption types mkDefault mkForce mkMerge mkIf;
in
{
  imports = [
    ./lxd
    ./wsl
    ./secrets.nix
    ./host-integration.nix
    ./init.nix
  ];

  options.codchi.driver = {
    name = mkOption {
      type = types.enum [ "wsl" "lxd" "none" ];
      internal = true;
    };
    iconCommand = mkOption {
      type = types.nullOr types.str;
      default = null;
      internal = true;
      description = ''
        Bash command to convert XDG desktop icons to ones that the host platform understands.
        Receives arguments `$ICON_PATH` and `$APP_NAME`. Result should be
        written to `codchi/icons/$APP_NAME.<extension>`.

        Use `null` to disable.
      '';
    };
    containerCfg = mkOption {
      type = types.attrs;
      internal = true;
      default = { };
    };
  };

  config = mkMerge [

    # Nix(OS) stuff managed by codchi
    {
      system.build.codchi.container =
        if (config.codchi.driver.name != "none") then
          (import ../../container { inherit inputs pkgs lib; }
            {
              config = lib.recursiveUpdate
                config.codchi.driver.containerCfg
                {
                  machine = {
                    enable = true;
                    driver.${config.codchi.driver.name}.enable = true;
                  };
                };
            }).config.build.tarball
        else null;

      systemd = {
        services = {
          # Create files required by the driver
          "create-files" = mkIf (config.codchi.driver.name != "none")
            {
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];
              serviceConfig.Type = "oneshot";
              script = /* bash */ ''
                ( cd / &&
                  ${lib.getExe config.system.build.codchi.container.passthru.createFiles}
                )
              '';
            };

          nix-daemon.enable = mkForce false;
          nix-gc.enable = mkForce false;
          nix-optimize.enable = mkForce false;
        };

        # Make sure all profiles are recorded as gcroots
        tmpfiles.rules = [
          "L+ /nix/var/nix/gcroots/profiles 0755 root root - /nix/var/nix/profiles"
        ];

        sockets.nix-daemon.enable = mkForce false;
      };

      # disable nixos-rebuild
      system.disableInstallerTools = mkForce true;

      environment.variables.NIX_REMOTE = "daemon";
      # Setup nix flakes
      nix = {
        package =
          if lib.versionAtLeast config.system.stateVersion "24.11"
          then pkgs.nix
          else pkgs.nixFlakes;
        extraOptions = ''
          experimental-features = nix-command flakes
        '';
        registry.nixpkgs.flake = inputs.nixpkgs;
        nixPath = [ "nixpkgs=/etc/channels/nixpkgs" ];

        settings = {
          extra-substituters = [ "https://codchi.cachix.org" ];
          trusted-public-keys = [ "codchi.cachix.org-1:dVwdzogJgZO2x8kPKW02HNt2dpd/P/z46pY465MkokY=" ];
        };
      };
      environment.etc."channels/nixpkgs".source = inputs.nixpkgs;
    }


    # general codchi machine management
    {
      boot.isContainer = true;

      # source codchi secrets
      environment.extraInit = /* bash */ ''
        source /etc/codchi-env
      '';

      # useful for usbip but adds a dependency on various firmwares which are combined over 300 MB big
      services.udev.enable = mkDefault false;

      networking.firewall.enable = mkDefault false;

      systemd = {
        # Don't allow emergency mode, because we don't have a console.
        enableEmergencyMode = false;
        # systemd-oomd requires cgroup pressure info which WSL doesn't have
        oomd.enable = false;
      };

      users.mutableUsers = mkForce false;
      users.users.${consts.machine.USER} = {
        isNormalUser = mkForce true;
        createHome = mkForce true;
        home = mkForce "/home/${consts.machine.USER}";
        uid = mkForce 1000;
        extraGroups = [ "wheel" ];
        initialPassword = consts.machine.USER;
      };
      security.sudo.wheelNeedsPassword = mkDefault false;

      hardware =
        if lib.versionAtLeast config.system.stateVersion "24.11"
        then { graphics.enable = lib.mkDefault true; }
        else { opengl.enable = lib.mkDefault true; };
      # powerManagement.enable = false;

    }

    # Desktop stuff
    {
      xdg = mkDefault {
        autostart.enable = true;
        menus.enable = true;
        mime.enable = true;
        icons.enable = true;
      };
      fonts.enableDefaultPackages = mkDefault true;

      environment = {

        systemPackages = [
          pkgs.xdg-utils
          pkgs.nixos-icons # needed for gnome and pantheon about dialog, nixos-manual and maybe more
        ];

        # Copy .desktop files and corresponding icons (converted to .ico) to
        # /run/current-system/sw/share/codchi. Copy instead of symlink them, so
        # that drivers like WSL can see them.
        extraSetup = mkIf (config.codchi.driver.iconCommand != null) /* bash */ ''
          pushd $out/share

          mkdir -p codchi/{icons,applications}

          for app_path in applications/*.desktop; do
            if grep -q ^NoDisplay=true "$app_path" || grep -q ^Hidden=true "$app_path"; then
              echo "Skipping hidden $app_path" >&2
            else
              APP_NAME="$(basename "$app_path" | sed 's/\.desktop//')"
              ICON_NAME="$(grep Icon "$app_path" | sed 's/Icon=//')"
              DIRS=
              [ -d icons ] && DIRS="$DIRS icons"
              [ -d pixmaps ] && DIRS="$DIRS pixmaps"
              ICON_PATH="$(find -L $DIRS -name "$ICON_NAME.*" | sort -rV | head -n1)"
              if [ ! -z "$ICON_PATH" ]; then
                ${config.codchi.driver.iconCommand}
              fi
              cp "$app_path" codchi/applications
            fi
          done

          popd
        '';
      };

    }
  ];

}
