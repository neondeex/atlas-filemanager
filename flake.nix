{
  description = "Atlas, a modern file manager built with Rust and GPUI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        atlas = pkgs.callPackage ./nix/package.nix { };
        # For contributors, not for installing next to `atlas` (same bin/atlas): the perf-log frame-timing build
        # without LTO and with 16 codegen units, like the release-perf profile, so it also builds in far less
        # memory. Run: ATLAS_PERF=1 ATLAS_PERF_EVENTS=1 ATLAS_PERF_MS=4 GPUI_FRAME_LOG=0 atlas
        atlas-perf = (atlas.override { features = [ "perf-log" ]; }).overrideAttrs {
          pname = "atlas-perf";
          CARGO_PROFILE_RELEASE_LTO = "off";
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
        };
        default = atlas;
      });

      devShells = forAll (pkgs: {
        default =
          let
            atlas = self.packages.${pkgs.stdenv.hostPlatform.system}.atlas;
          in
          pkgs.mkShell {
            inputsFrom = [ atlas ];
            packages = [
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
            ];
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath atlas.runtimeLibs;
          };
      });

      formatter = forAll (pkgs: pkgs.nixfmt);
    };
}
