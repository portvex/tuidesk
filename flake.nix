{
  description = "tuidesk: a KDE Plasma style desktop that runs inside a terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      manifest = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;

      mkTuidesk =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          inherit (manifest) version;
          # Only what the build needs, so editing docs or packaging does not trigger a rebuild.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./config
              ./man
              ./tests
              ./xtask
              ./README.md
              ./LICENSE
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "tuidesk" ];
          # The integration tests drive tmux and a background server; the sandbox has neither
          # a tmux nor a writable home, so only the unit tests run here.
          cargoTestFlags = [ "--package" "tuidesk" "--lib" ];
          nativeBuildInputs = [ pkgs.installShellFiles ];
          postInstall = ''
            installManPage man/tuidesk.1 man/tuidesk-config.5 man/tuidesk-lua.7
          '';
          meta = {
            inherit (manifest) description homepage;
            license = pkgs.lib.licenses.mit;
            mainProgram = "tuidesk";
            platforms = systems;
          };
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        tuidesk = mkTuidesk pkgs;
        default = tuidesk;
      });

      overlays.default = final: _prev: { tuidesk = mkTuidesk final; };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.tuidesk ];
          packages = with pkgs; [
            cargo-deny
            cargo-audit
            cargo-llvm-cov
            cargo-zigbuild
            clippy
            mandoc
            rustfmt
            tmux
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
