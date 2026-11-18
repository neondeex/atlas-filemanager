#!/usr/bin/env bash
# Inside a fresh Debian 13 or Arch Linux cloud VM (as root): light desktop for a small VM. sway (Wayland,
# no XWayland) started by tty1 autologin of user "atlas", Nautilus for clipboard interop, then Atlas.
#   provision-sway.sh /path/to/atlas.deb|atlas.pkg.tar.zst
set -euxo pipefail
pkg=${1:?Atlas package}
. /etc/os-release
case $ID in
  debian)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends sway swaybg foot nautilus gvfs dbus-user-session \
      wl-clipboard fuse3 python3 mesa-vulkan-drivers libgl1-mesa-dri fonts-dejavu-core xdg-desktop-portal-gtk
    apt-get install -y "$pkg"
    profile=/home/atlas/.profile ;;
  arch)
    pacman -Syu --noconfirm --needed sway foot nautilus gvfs wl-clipboard fuse3 python mesa vulkan-swrast \
      vulkan-icd-loader ttf-dejavu xdg-desktop-portal-gtk
    pacman -U --noconfirm "$pkg"
    profile=/home/atlas/.bash_profile ;;
  *) echo "unsupported $ID"; exit 1 ;;
esac
mkdir -p /etc/systemd/system/getty@tty1.service.d
cat > /etc/systemd/system/getty@tty1.service.d/autologin.conf <<CONF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin atlas --noclear %I \$TERM
CONF
install -d -o atlas -g atlas /home/atlas/.config /home/atlas/.config/sway
cat > /home/atlas/.config/sway/config <<CONF
include /etc/sway/config
output * resolution 1366x768
xwayland disable
CONF
chown atlas:atlas /home/atlas/.config/sway/config
[[ -f $profile ]] || install -o atlas -g atlas /dev/null "$profile"
cat >> "$profile" <<'CONF'
[ "$(tty)" = /dev/tty1 ] && [ -z "$WAYLAND_DISPLAY" ] && exec sway > /tmp/sway.log 2>&1
CONF
