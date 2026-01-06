{
  lib,
  rustPlatform,
}:
let
  fs = lib.fileset;

  files = fs.unions [
    ./src
    ./Cargo.lock
    ./Cargo.toml
    ./systemd
  ];
in
rustPlatform.buildRustPackage {
  pname = "dircacher";
  version = "0.5.1";

  src = fs.toSource {
    root = ./.;
    fileset = files;
  };

  cargoLock.lockFile = ./Cargo.lock;

  postInstall = ''
    install -Dm644 systemd/dircacher.service "$out/share/systemd/user"
  '';
}
