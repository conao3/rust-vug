{
  description = "vug: a declarative package manager and system builder";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    leanfran = {
      url = "github:conao3/lean-leanfran/420a7dce66dda3cbfd03b884d453cb8de63a0fc6";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];
      imports = [ inputs.treefmt-nix.flakeModule ];
      perSystem =
        { pkgs, system, ... }:
        let
          leanfran = inputs.leanfran.packages.${system}.leanfran;
        in
        {
          treefmt = {
            projectRootFile = "flake.nix";
            programs.nixfmt.enable = true;
            programs.rustfmt.enable = true;
          };
          devShells.default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.lean4
              pkgs.gnumake
              pkgs.bubblewrap
              leanfran
            ];
            LEANFRAN_CLI = "${leanfran}/bin/leanfran";
          };
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "vug";
            version = "0.2.29";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            LEANFRAN_OFFLINE = "1";
            nativeBuildInputs = [
              pkgs.makeWrapper
              pkgs.pkg-config
            ];
            buildInputs = [ pkgs.zstd ];
            nativeCheckInputs = [ pkgs.gitMinimal ];
            postInstall = ''
              wrapProgram $out/bin/vug \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bubblewrap ]} \
                --suffix PATH : ${
                  pkgs.lib.makeBinPath [
                    pkgs.gitMinimal
                    pkgs.openssh
                  ]
                }
            '';
            meta = {
              mainProgram = "vug";
              license = pkgs.lib.licenses.asl20;
            };
          };
          packages.static = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
            pname = "vug";
            version = "0.2.29";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            LEANFRAN_OFFLINE = "1";
            nativeBuildInputs = [ pkgs.pkgsStatic.buildPackages.pkg-config ];
            buildInputs = [ pkgs.pkgsStatic.zstd ];
            VUG_BUNDLED_BWRAP = "1";
            doCheck = false;
            postInstall = ''
              install -m 0555 ${pkgs.pkgsStatic.bubblewrap}/bin/bwrap $out/bin/vug-bwrap
            '';
            meta.license = pkgs.lib.licenses.asl20;
          };
          packages.release =
            pkgs.runCommand "vug-release"
              {
                static = inputs.self.packages.${system}.static;
                archive = "vug-${inputs.self.packages.${system}.static.version}-x86_64-linux-musl.tar.gz";
              }
              ''
                mkdir $out
                tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 \
                  -C $static/bin -c vug vug-bwrap | gzip -9n > $out/$archive
              '';
        };
    };
}
