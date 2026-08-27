{
  description = "mist - Fast, minimal Windows launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      system = "x86_64-linux";

      pkgs = import nixpkgs {
        inherit system;

        overlays = [
          rust-overlay.overlays.default
        ];
      };

      rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
          "clippy"
        ];

        targets = [
          "x86_64-pc-windows-gnu"
        ];
      };

    in
    {
      devShells.${system}.default = pkgs.mkShell {

        packages = with pkgs; [
          rustToolchain
          zig
          cargo-zigbuild
          pkgsCross.mingwW64.stdenv.cc
        ];
      };
    };
}
