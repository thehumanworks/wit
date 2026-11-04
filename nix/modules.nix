# NixOS and Home Manager modules: `programs.wit.enable = true;`.
{ self }:

let
  # `packagesPath` / `variablesPath`: where the target module system takes
  # installed packages and session environment variables.
  mkModule =
    { packagesPath, variablesPath }:
    {
      config,
      lib,
      pkgs,
      ...
    }:
    let
      cfg = config.programs.wit;
    in
    {
      options.programs.wit = {
        enable = lib.mkEnableOption "wit, a CLI and MCP server for reading GitHub repositories";

        package = lib.mkOption {
          type = lib.types.package;
          default = self.packages.${pkgs.stdenv.hostPlatform.system}.wit;
          defaultText = lib.literalExpression "wit.packages.\${pkgs.stdenv.hostPlatform.system}.wit";
          description = "The wit package to install (provides `wit` and `wit-mcp`).";
        };

        cacheUrl = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "off";
          description = ''
            Value for `WIT_CACHE_URL`: the shared cloud pack cache that disk
            reads try before cloning from GitHub. `null` keeps the built-in
            default (the hosted cache); `"off"` disables it.
          '';
        };
      };

      config = lib.mkIf cfg.enable (
        lib.mkMerge [
          (lib.setAttrByPath packagesPath [ cfg.package ])
          (lib.mkIf (cfg.cacheUrl != null) (
            lib.setAttrByPath (variablesPath ++ [ "WIT_CACHE_URL" ]) cfg.cacheUrl
          ))
        ]
      );
    };
in
{
  nixos = mkModule {
    packagesPath = [
      "environment"
      "systemPackages"
    ];
    variablesPath = [
      "environment"
      "variables"
    ];
  };
  homeManager = mkModule {
    packagesPath = [
      "home"
      "packages"
    ];
    variablesPath = [
      "home"
      "sessionVariables"
    ];
  };
}
