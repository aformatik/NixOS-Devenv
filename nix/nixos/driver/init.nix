{ lib, pkgs, config, ... }:
let
  inherit (lib) types mkOption;
in
{
  options.codchi.initScript = mkOption {
    description = ''
      A bash script which will run once on machine creation (init or clone) as
      the default codchi user. Afterwards it can be run manually via
      `codchi-init`.
    '';
    default = "";
    type = types.nullOr types.lines;
    example = ''
      cd $HOME
      git clone https://github.com/my/cool-repo
    '';
  };

  config = {
    environment.systemPackages = [
      (pkgs.writeShellScriptBin "codchi-init" config.codchi.initScript)
    ];
  };

}
