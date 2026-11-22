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
      url = "github:conao3/lean-leanfran/1bb464c6d419da8037698053f52dd23476a042d9";
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
          leanfran = inputs.leanfran.packages.${system}.rust-compiler;
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
            version = "0.1.14";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            LEANFRAN_OFFLINE = "1";
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postInstall = ''
              wrapProgram $out/bin/vug --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bubblewrap ]}
            '';
            meta = {
              mainProgram = "vug";
              license = pkgs.lib.licenses.asl20;
            };
          };
          packages.static = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
            pname = "vug";
            version = "0.1.14";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            LEANFRAN_OFFLINE = "1";
            doCheck = false;
            meta.license = pkgs.lib.licenses.asl20;
          };
        };
    };
}
