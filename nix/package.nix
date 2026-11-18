# Atlas for Nix. Built by the flake (`nix build`); also usable with callPackage.
{
  lib,
  rustPlatform,
  pkg-config,
  fontconfig,
  freetype,
  libGL,
  libxkbcommon,
  openssl,
  sqlite,
  vulkan-loader,
  wayland,
  libx11,
  libxcb,
  librsvg,
  zstd,
  # Extra Cargo features, e.g. [ "perf-log" ] for a frame-timing build (ATLAS_PERF=1).
  features ? [ ],
}:

let
  # GPUI opens these at run time (dlopen); they go on the binary's rpath, so no wrapper script renames
  # the process (pgrep atlas, the window's app id and the desktop entry keep working).
  runtimeLibs = [
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    libx11
    libxcb
  ];
in
rustPlatform.buildRustPackage {
  pname = "atlas";
  # Release builds get their number from CI (ATLAS_RELEASE_VERSION); a source build reports Cargo.toml's.
  inherit ((lib.importTOML ../Cargo.toml).package) version;

  # Without the Nix files and target/: a change to only the comments or formatting of the Nix files does not
  # rebuild Atlas; changing the nixpkgs pin or the build inputs does.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.difference (lib.fileset.fromSource (lib.cleanSource ../.)) (
      lib.fileset.unions [
        ../flake.nix
        ../flake.lock
        ./.
        (lib.fileset.maybeMissing ../target)
      ]
    );
  };
  cargoLock = {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  # The same binaries the .deb, .rpm and Arch packages ship; Atlas finds the helpers next to itself.
  cargoBuildFlags = [
    "--package=atlas"
    "--package=atlas-search-service"
    "--package=atlas-cloud-extension"
    "--package=atlas-icon-extension"
  ];
  buildFeatures = features;
  # The test suite needs a display and network fixtures; CI runs it.
  doCheck = false;

  nativeBuildInputs = [
    librsvg
    pkg-config
    rustPlatform.bindgenHook
  ];
  # nixpkgs' cargo hooks make libsqlite3-sys and zstd-sys link these system libraries (pkg-config).
  buildInputs = [
    fontconfig
    freetype
    openssl
    sqlite
    zstd
  ]
  ++ runtimeLibs;

  # The extra files of the Linux packages (packaging/linux/nfpm.yaml), except the AppStream metadata that
  # scripts/linux_release.py generates. The icon is 500x500, so it is scaled to 512 as that script does.
  postInstall = ''
    install -Dm644 packaging/linux/atlas.desktop $out/share/applications/atlas.desktop
    printf '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 1 1"><image width="1" height="1" xlink:href="data:image/png;base64,%s"/></svg>' \
      "$(base64 -w0 packaging/icons/atlas.png)" > atlas-icon.svg
    mkdir -p $out/share/icons/hicolor/512x512/apps
    rsvg-convert --width 512 --height 512 --output $out/share/icons/hicolor/512x512/apps/atlas.png atlas-icon.svg
    install -Dm644 LICENSE $out/share/licenses/atlas/LICENSE
    install -Dm644 legal/THIRD-PARTY-NOTICES.txt $out/share/doc/atlas/THIRD-PARTY-NOTICES.txt
    install -Dm644 CHANGELOG.md $out/share/doc/atlas/CHANGELOG.md
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/atlas
  '';

  # For the dev shell's LD_LIBRARY_PATH.
  passthru = { inherit runtimeLibs; };

  meta = {
    description = "Modern file manager built with Rust and GPUI";
    homepage = "https://atlasnative.com";
    license = lib.licenses.agpl3Plus;
    mainProgram = "atlas";
    platforms = lib.platforms.linux;
  };
}
