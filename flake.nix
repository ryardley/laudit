{
  description = "laudit — guided codebase tours";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];
    forAllSystems = nixpkgs.lib.genAttrs systems;

    cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    version = cargoToml.workspace.package.version;

    mkLaudit = pkgs:
      pkgs.rustPlatform.buildRustPackage {
        pname = "laudit";
        inherit version;

        src = ./.;

        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = ["--workspace"];

        nativeBuildInputs = [pkgs.git];

        meta = with pkgs.lib; {
          description = "Guided codebase tours";
          license = licenses.mit;
          mainProgram = "laudit";
          platforms = platforms.linux;
        };
      };

    mkLauditNvim = pkgs:
      pkgs.stdenvNoCC.mkDerivation {
        pname = "laudit-nvim";
        inherit version;

        src = ./.;

        dontBuild = true;

        installPhase = ''
          runHook preInstall
          mkdir -p $out
          cp -r lua plugin $out/
          runHook postInstall
        '';

        meta = with pkgs.lib; {
          description = "Neovim plugin for laudit";
          license = licenses.mit;
          platforms = platforms.linux;
        };
      };

    mkTk = pkgs: let
      ticketSrc = pkgs.fetchFromGitHub {
        owner = "wedow";
        repo = "ticket";
        rev = "194b71a8bbc3771da1ce9f579395937c976bbddc";
        hash = "sha256-S7nA8IiOCYqA6ic1b5MkE7fUCtViUiEUVeJ4HV1re2o=";
      };
    in
      pkgs.writeShellApplication {
        name = "tk";
        runtimeInputs = with pkgs; [coreutils gnused gawk gnugrep ripgrep jq git];
        text = ''exec ${pkgs.bash}/bin/bash ${ticketSrc}/ticket "$@"'';
      };

    # The nixpkgs `opencode` package builds its node_modules from source, which
    # is slow and not available from the binary cache. Use the official
    # prebuilt release binary instead so `nix develop` stays fast.
    mkOpencode = pkgs: let
      sources = {
        x86_64-linux = {
          url = "https://github.com/anomalyco/opencode/releases/download/v2.2.6/opencode-linux-x64.tar.gz";
          hash = "sha256-CLsh1jJs50DZHM5+voimU2kz9uHjA1O5GgawHyXaMGw=";
        };
        aarch64-linux = {
          url = "https://github.com/anomalyco/opencode/releases/download/v2.2.6/opencode-linux-arm64.tar.gz";
          hash = "sha256-8tJ2AMNn5FBX1qFvalwqqNd7QamxDUvD2EnC+Ihw6UQ=";
        };
      };
      source =
        sources.${pkgs.stdenv.hostPlatform.system}
        or (throw "opencode: unsupported system ${pkgs.stdenv.hostPlatform.system}");
    in
      pkgs.stdenvNoCC.mkDerivation {
        pname = "opencode";
        version = "2.2.6";

        src = pkgs.fetchurl {inherit (source) url hash;};

        nativeBuildInputs = [pkgs.autoPatchelfHook];
        buildInputs = [pkgs.stdenv.cc.cc.lib];

        dontConfigure = true;
        dontBuild = true;

        unpackPhase = ''
          runHook preUnpack
          tar -xzf $src
          runHook postUnpack
        '';
        sourceRoot = ".";

        installPhase = ''
          runHook preInstall
          install -Dm755 opencode $out/bin/opencode
          runHook postInstall
        '';

        meta = with pkgs.lib; {
          description = "AI coding agent built for the terminal";
          homepage = "https://github.com/anomalyco/opencode";
          license = licenses.mit;
          mainProgram = "opencode";
          platforms = platforms.linux;
        };
      };
  in {
    packages = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
      laudit = mkLaudit pkgs;
    in {
      default = laudit;
      laudit = laudit;
      laudit-nvim = mkLauditNvim pkgs;
    });

    apps = forAllSystems (system: let
      laudit = self.packages.${system}.laudit;
    in {
      default = {
        type = "app";
        program = "${laudit}/bin/laudit";
        meta.description = "Guided codebase tours";
      };
      laudit = {
        type = "app";
        program = "${laudit}/bin/laudit";
        meta.description = "Guided codebase tours";
      };
    });

    devShells = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustfmt
          clippy
          rust-analyzer
          git
          pkg-config
          (mkTk pkgs)
          (mkOpencode pkgs)
        ];
      };
    });

    overlays.default = final: _prev: {
      laudit = mkLaudit final;
    };
  };
}
