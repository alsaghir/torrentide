{
  description = "Torrentide — BitTorrent desktop client written in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # Becomes the GUI crate's package name once that crate exists (M5).
      name = "torrentide";
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      workspace = (lib.importTOML ./Cargo.toml).workspace.package;
      toolchainFile = ./rust-toolchain.toml;
      toolchainChannel = (lib.importTOML toolchainFile).toolchain.channel;

      perSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            p: p.rust-bin.fromRustupToolchainFile toolchainFile
          );

          src = lib.fileset.toSource {
            root = ./.;
            fileset = craneLib.fileset.commonCargoSources ./.;
          };
          nixSrc = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.fileFilter (file: file.hasExt "nix") ./.;
          };

          commonArgs = {
            inherit src;
            inherit (workspace) version;
            pname = name;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          checks = {
            fmt = craneLib.cargoFmt { inherit (commonArgs) src pname version; };
            clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );
            # `pass` while the workspace has no tests yet; nextest fails on an empty run by default.
            nextest = craneLib.cargoNextest (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoNextestExtraArgs = "--no-tests=pass";
              }
            );
            nixfmt = pkgs.runCommand "${name}-nixfmt" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
              find ${nixSrc} -name '*.nix' -exec nixfmt --check {} +
              touch $out
            '';
            # Cargo cannot read rust-toolchain.toml, so `rust-version` is the one unavoidable copy.
            rust-version = pkgs.runCommand "${name}-rust-version" { } (
              if lib.versions.majorMinor toolchainChannel == workspace.rust-version then
                "touch $out"
              else
                ''
                  echo "Cargo.toml rust-version (${workspace.rust-version}) != rust-toolchain.toml channel (${toolchainChannel})" >&2
                  exit 1
                ''
            );
          };
        in
        {
          inherit checks;
          # `checks` pulls in every check's build inputs (toolchain, cargo-nextest) without re-listing them.
          devShells.default =
            let
              # Mason's prebuilt codelldb does not run on NixOS; LazyVim finds this one on PATH.
              codelldb = pkgs.vscode-extensions.vadimcn.vscode-lldb;
            in
            craneLib.devShell {
              inherit checks;
              name = "${name}-devshell";
              shellHook = ''
                mkdir -p .sdk/bin
                ln -sf "${codelldb}/share/vscode/extensions/vadimcn.vscode-lldb/adapter/codelldb" .sdk/bin/codelldb
                export PATH="$PWD/.sdk/bin:$PATH"
                echo "${name} dev shell: $(rustc --version)"
              '';
            };
          formatter = pkgs.nixfmt-tree;
        };

      bySystem = lib.genAttrs systems perSystem;
    in
    {
      checks = lib.mapAttrs (_: outputs: outputs.checks) bySystem;
      devShells = lib.mapAttrs (_: outputs: outputs.devShells) bySystem;
      formatter = lib.mapAttrs (_: outputs: outputs.formatter) bySystem;
    };
}
