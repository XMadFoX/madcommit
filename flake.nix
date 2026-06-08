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
          gcc
        ];

        RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        RUSTFLAGS = "-C link-arg=-Wl,-rpath=${pkgs.lib.makeLibraryPath [ pkgs.openssl ]}";
        NIX_LDFLAGS = "-rpath ${pkgs.lib.makeLibraryPath [ pkgs.openssl ]}";
        CC = "${pkgs.gcc}/bin/gcc";
        CXX = "${pkgs.gcc}/bin/g++";
        OPENSSL_DIR = "${pkgs.openssl.dev}";
        OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
        OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
        OPENSSL_NO_VENDOR = "1";
        };
    };
}
