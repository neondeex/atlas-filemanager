#!/usr/bin/env bash
# Builds $ATLAS_VM_ROOT/atlas-nixos/base.qcow2 (default ~/vm): NixOS (unstable, same nixpkgs as Atlas's flake.lock) with GNOME on
# Wayland, autologin "atlas", ssh key, flakes. Built in the nixos/nix container with /dev/kvm; the Nix
# store lives in the docker volume atlas-nix-store.   nixos-image.sh FLAKE_LOCK
set -euo pipefail
lock=$(realpath "${1:?path to Atlas flake.lock}")
dir=${ATLAS_VM_ROOT:-$HOME/vm}/atlas-nixos; mkdir -p "$dir/image"; cd "$dir"
[[ -f id_ed25519 ]] || ssh-keygen -q -t ed25519 -N '' -f id_ed25519
cp "$lock" image/flake.lock
cat > image/flake.nix <<EOF
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { nixpkgs, ... }: {
    nixosConfigurations.vm = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [ ({ config, lib, pkgs, modulesPath, ... }: {
        imports = [ "\${modulesPath}/profiles/qemu-guest.nix" ];
        system.build.qcow2 = import "\${modulesPath}/../lib/make-disk-image.nix" {
          inherit lib config pkgs; format = "qcow2"; diskSize = "auto";
          additionalSpace = "1G"; partitionTableType = "legacy";
        };
        boot.loader.grub.device = "/dev/vda";
        boot.growPartition = true;
        fileSystems."/" = { device = "/dev/disk/by-label/nixos"; fsType = "ext4"; autoResize = true; };
        networking.hostName = "atlas-nixos";
        time.timeZone = "UTC";

        services.displayManager.gdm.enable = true;
        services.desktopManager.gnome.enable = true;
        services.displayManager.autoLogin = { enable = true; user = "atlas"; };
        systemd.services."getty@tty1".enable = false;
        systemd.services."autovt@tty1".enable = false;
        services.gnome.gnome-initial-setup.enable = false;
        environment.gnome.excludePackages = [ pkgs.gnome-tour ];
        programs.dconf.profiles.user.databases = [{ settings = {
          "org/gnome/desktop/session".idle-delay = lib.gvariant.mkUint32 0;
          "org/gnome/desktop/screensaver".lock-enabled = false;
          "org/gnome/desktop/notifications".show-banners = false;
          "org/gnome/settings-daemon/plugins/power".sleep-inactive-ac-type = "nothing";
          "org/gnome/shell".welcome-dialog-last-shown-version = "999";
        }; }];
        hardware.graphics.enable = true;

        users.users.atlas = {
          isNormalUser = true; extraGroups = [ "wheel" ]; password = "atlas";
          openssh.authorizedKeys.keys = [ "$(cat id_ed25519.pub)" ];
        };
        security.sudo.wheelNeedsPassword = false;
        services.openssh.enable = true;
        nix.settings = { experimental-features = [ "nix-command" "flakes" ]; trusted-users = [ "atlas" ]; };
        environment.systemPackages = with pkgs; [ git wl-clipboard python3 fuse3 procps psmisc file ];
        system.stateVersion = "26.05";
      }) ];
    };
  };
}
EOF
docker run --rm --device /dev/kvm -v atlas-nix-store:/nix -v "$dir/image":/image nixos/nix:latest sh -c '
  printf "experimental-features = nix-command flakes\nsystem-features = kvm nixos-test benchmark big-parallel\nmax-jobs = 4\ncores = 4\n" >> /etc/nix/nix.conf
  cp -r /image /w && cd /w && nix build -L .#nixosConfigurations.vm.config.system.build.qcow2 2>&1 | tail -20 &&
  cp -L result/nixos.qcow2 /image/base.qcow2 && chmod 644 /image/base.qcow2'
mv image/base.qcow2 base.qcow2
qemu-img create -q -f qcow2 -b base.qcow2 -F qcow2 disk.qcow2 60G
ls -la "$dir"
