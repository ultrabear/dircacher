{
  inputs.nixpkgs.url = "https://nixos.org/channels/nixos-unstable/nixexprs.tar.xz";

  outputs =
    {
      self,
      nixpkgs,
    }:
    {
      overlays.default = import self;

      packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.callPackage ./package.nix { };
    };
}
