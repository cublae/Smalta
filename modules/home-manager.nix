self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.smalta;
  json = pkgs.formats.json { };
in
{
  options.programs.smalta = {
    enable = lib.mkEnableOption "Smalta, a pixel desktop shell for niri and Hyprland";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "smalta.packages.\${system}.default";
      description = "The Smalta package to use.";
    };

    settings = lib.mkOption {
      inherit (json) type;
      default = { };
      example = lib.literalExpression ''
        {
          bar = {
            position = "top";
            modules.end = [ "tray" "keyboard-layout" "system-indicators" "notifications" ];
          };
          theme.accent = "#e0607e";
          weather.location = "Moscow";
          desktop.enabled = true;
        }
      '';
      description = ''
        Written to {file}`~/.config/smalta/config.json`; see the README for
        the keys Smalta reads.
      '';
    };

    systemd.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Start Smalta from a systemd user service bound to
        graphical-session.target, which also restarts it if it ever exits.
        Turn this off to start it from the compositor's own autostart.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."smalta/config.json" = lib.mkIf (cfg.settings != { }) {
      source = json.generate "smalta-config.json" cfg.settings;
    };

    systemd.user.services.smalta = lib.mkIf cfg.systemd.enable {
      Unit = {
        Description = "Smalta desktop shell";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
        ConditionEnvironment = [ "WAYLAND_DISPLAY" ];
        X-Restart-Triggers = lib.optional (cfg.settings != { }) config.xdg.configFile."smalta/config.json".source;
      };
      Service = {
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        RestartSec = 1;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
