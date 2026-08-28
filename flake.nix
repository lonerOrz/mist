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
      devShells.${system}.default = pkgs.pkgsCross.mingwW64.mkShell {
        nativeBuildInputs = [
          rustToolchain
          pkgs.zig
          pkgs.cargo-zigbuild
        ];

        buildInputs = [
          pkgs.pkgsCross.mingwW64.windows.pthreads
        ];

        CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
      };
    };
}
