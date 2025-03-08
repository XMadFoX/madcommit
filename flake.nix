{
  description = "Dev env";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = { nixpkgs, ... }: let
    system = builtins.currentSystem;

    in {
      devShells."${system}".default = let
        pkgs = import nixpkgs {
          inherit system;
        };

      in pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          clippy
          rust-analyzer
          rustfmt
          git
          toybox
          openssl
          pkg-config
        ];

        RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
    };
}
