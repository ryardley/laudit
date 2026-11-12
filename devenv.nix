{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: let
  ticketSrc = pkgs.fetchFromGitHub {
    owner = "wedow";
    repo = "ticket";
    rev = "194b71a8bbc3771da1ce9f579395937c976bbddc";
    hash = "sha256-S7nA8IiOCYqA6ic1b5MkE7fUCtViUiEUVeJ4HV1re2o=";
  };
  tk = pkgs.writeShellApplication {
    name = "tk";
    runtimeInputs = with pkgs; [coreutils gnused gawk gnugrep ripgrep jq git];
    text = ''exec ${pkgs.bash}/bin/bash ${ticketSrc}/ticket "$@"'';
  };
  laudit = inputs.laudit.packages.${pkgs.stdenv.hostPlatform.system}.laudit;
in {
  languages.rust.enable = true;
  packages = [tk laudit];
}
