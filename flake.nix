{
  description = "Dev env";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
        };

      packageFor =
        system:
        let
          pkgs = pkgsFor system;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "madcommit";
          version = "0.1.0";

          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl ];

          postPatch = ''
            substituteInPlace src/config.rs \
              --replace-fail 'template_path: "template.md".to_string()' \
                             'template_path: "'"$out"'/share/madcommit/template.md".to_string()'
          '';

          postInstall = ''
            install -Dm644 template.md $out/share/madcommit/template.md
          '';

          OPENSSL_NO_VENDOR = "1";
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          madcommit = packageFor system;
        in
        {
          inherit madcommit;
          default = madcommit;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${packageFor system}/bin/madcommit";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          libPath = pkgs.lib.makeLibraryPath [ pkgs.openssl ];
        in
        {
          default = pkgs.mkShell {
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
            RUSTFLAGS = "-C link-arg=-Wl,-rpath=${libPath}";
            NIX_LDFLAGS = "-rpath ${libPath}";
            CC = "${pkgs.gcc}/bin/gcc";
            CXX = "${pkgs.gcc}/bin/g++";
            OPENSSL_DIR = "${pkgs.openssl.dev}";
            OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
            OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
            OPENSSL_NO_VENDOR = "1";
          };
        }
      );
    };
}
