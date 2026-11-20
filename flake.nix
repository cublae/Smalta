{
  description = "Smalta — a pixel desktop shell for niri and Hyprland, in Rust";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        smalta = pkgs.callPackage ./nix/package.nix { };
        default = smalta;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          name = "smalta-dev";
          # The same fonts the package bakes in, for `cargo run`.
          SMALTA_FONT = "${pkgs.spleen}/share/fonts/misc/spleen-8x16.bdf";
          SMALTA_UNIFONT = "${pkgs.unifont.doc}/share/unifont/unifont_all.hex";
          SMALTA_UNIFONT_UPPER = "${pkgs.runCommand "unifont-upper.hex" { } ''
            tar xzf ${pkgs.unifont.src} --wildcards --to-stdout '*/font/precompiled/unifont_upper-*.hex' > $out
          ''}";
          SMALTA_EMOJI = "${pkgs.runCommand "noto-emoji-png" { } ''
            cp -r ${pkgs.noto-fonts-color-emoji.src}/png/128 $out
            chmod -R u+w $out
            cp ${pkgs.noto-fonts-color-emoji.src}/third_party/region-flags/waved-svg/*.svg $out/
          ''}";

          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
          ];
        };
      });

      homeManagerModules = {
        smalta = import ./modules/home-manager.nix self;
        default = self.homeManagerModules.smalta;
      };

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
