{
  outputs = { ... }: {
    nixosModules.default = import ./configuration.nix;
  };
}
