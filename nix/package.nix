{
  lib,
  rustPlatform,
  makeWrapper,
  spleen,
  unifont,
  runCommand,
  noto-fonts-color-emoji,
  pulseaudio,
  networkmanager,
  cliphist,
  cava,
  wl-clipboard,
  grim,
  slurp,
  curl,
  procps,
  coreutils,
}:

# The fonts are baked in at build time: Smalta draws every glyph itself, from
# Spleen's BDF, and falls back to Unifont's .hex for whatever Spleen lacks.
# Unifont's planes above the first, where the emoji are, come in nixpkgs only
# as an OTF; its source has them as .hex too, and that is what is read.
# Colour emoji are Noto's 128-pixel PNGs and its waved flags, as SVG, copied
# out of its source on their own: the rest of the repository is ten times
# their size.
# Neither needs fontconfig or any other library at run time, so the binary
# links nothing beyond libc.
#
# The programs it runs go on the wrapper's PATH, or the feature behind each
# silently stays dark: pactl for audio, nmcli for the network, cliphist and
# wl-clipboard for the clipboard, grim and slurp for Hyprland screenshots,
# curl for the weather and exchange rates, cava for the visualizer.
# systemctl, loginctl and gpu-screen-recorder are the system's own and are
# found after these.

rustPlatform.buildRustPackage {
  pname = "smalta";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: _type:
      let
        base = baseNameOf path;
      in
      !(lib.hasPrefix "result" base) && base != "target";
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  env = {
    SMALTA_FONT = "${spleen}/share/fonts/misc/spleen-8x16.bdf";
    SMALTA_UNIFONT = "${unifont.doc}/share/unifont/unifont_all.hex";
    SMALTA_UNIFONT_UPPER = runCommand "unifont-upper.hex" { } ''
      tar xzf ${unifont.src} --wildcards --to-stdout '*/font/precompiled/unifont_upper-*.hex' > $out
    '';
    SMALTA_EMOJI = runCommand "noto-emoji-png" { } ''
      cp -r ${noto-fonts-color-emoji.src}/png/128 $out
      chmod -R u+w $out
      cp ${noto-fonts-color-emoji.src}/third_party/region-flags/waved-svg/*.svg $out/
    '';
  };

  postInstall = ''
    wrapProgram $out/bin/smalta \
      --prefix PATH : ${
        lib.makeBinPath [
          pulseaudio
          networkmanager
          cliphist
          cava
          wl-clipboard
          grim
          slurp
          curl
          procps
          coreutils
        ]
      } \
      --suffix PATH : /run/current-system/sw/bin
  '';

  meta = {
    description = "A pixel desktop shell for niri and Hyprland";
    mainProgram = "smalta";
    platforms = lib.platforms.linux;
    license = lib.licenses.mit;
  };
}
