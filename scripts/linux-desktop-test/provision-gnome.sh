#!/usr/bin/env bash
# Inside a fresh Debian 13 or Fedora 44 cloud VM (as root): GNOME on Wayland with GDM autologin
# for user "atlas", no screen lock or idle blanking, then the Atlas release package.
#   provision-gnome.sh /path/to/atlas.deb|atlas.rpm
set -euxo pipefail
pkg=$1
. /etc/os-release
case $ID in
  debian)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y gnome-core gdm3 nautilus wl-clipboard fuse3 python3 dconf-cli
    apt-get install -y "$pkg"
    gdm=/etc/gdm3/daemon.conf ;;
  fedora)
    # unoconv needs libreoffice-pyuno, so it goes with LibreOffice.
    dnf -y install @workstation-product-environment -x 'libreoffice*' -x unoconv
    dnf -y install wl-clipboard fuse3 python3
    dnf -y install "$pkg"
    gdm=/etc/gdm/custom.conf ;;
  *) echo "unsupported $ID"; exit 1 ;;
esac
cat > "$gdm" <<CONF
[daemon]
AutomaticLoginEnable=true
AutomaticLogin=atlas
WaylandEnable=true
CONF
mkdir -p /etc/dconf/profile /etc/dconf/db/local.d
printf 'user-db:user\nsystem-db:local\n' > /etc/dconf/profile/user
cat > /etc/dconf/db/local.d/00-atlas-vm <<CONF
[org/gnome/desktop/session]
idle-delay=uint32 0
[org/gnome/desktop/screensaver]
lock-enabled=false
[org/gnome/desktop/lockdown]
disable-lock-screen=true
[org/gnome/desktop/notifications]
show-banners=false
[org/gnome/shell]
welcome-dialog-last-shown-version='999'
[org/gnome/settings-daemon/plugins/power]
sleep-inactive-ac-type='nothing'
CONF
dconf update
install -d -o atlas -g atlas /home/atlas/.config
echo yes > /home/atlas/.config/gnome-initial-setup-done
chown atlas:atlas /home/atlas/.config/gnome-initial-setup-done
systemctl set-default graphical.target
