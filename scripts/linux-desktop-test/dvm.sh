#!/usr/bin/env bash
# Desktop test VMs (QEMU/KVM) with a real display, driven over ssh and QMP.
#
#   dvm.sh DISTRO up             create on first run, boot, wait for ssh
#   dvm.sh DISTRO ssh [cmd]      run a command as user "atlas"
#   dvm.sh DISTRO put SRC DST / get SRC DST
#   dvm.sh DISTRO qmp ARGS...    qmp.py on this VM (key, type, click, wheel, shot)
#   dvm.sh DISTRO down           power off
#   dvm.sh DISTRO destroy        power off and delete the VM directory
#
# DISTRO: arch, debian (13), fedora (44), nixos (image built by nixos-image.sh).
# VMs live in $ATLAS_VM_ROOT/atlas-DISTRO (default ~/vm). DVM_CPUS / DVM_MEM size them (4 / 4096 MiB).
# The display is a 1366x768 virtio-vga screen nobody watches; screenshots come from QMP screendump.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
distro=${1:?distro}; shift
root=${ATLAS_VM_ROOT:-$HOME/vm}
dir=$root/atlas-$distro
url='' base=base.qcow2 size=30G
case $distro in
  arch) port=2222 base=arch.qcow2 size=40G
    url=https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2 ;;
  debian) port=2223
    url=https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2 ;;
  fedora) port=2224
    url=https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2 ;;
  nixos) port=2225 ;;
  *) echo "unknown distro $distro" >&2; exit 2 ;;
esac
cpus=${DVM_CPUS:-4} mem=${DVM_MEM:-4096}
ssh_opts=(-i "$dir/id_ed25519" -p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
  -o LogLevel=ERROR -o ConnectTimeout=5 -o ServerAliveInterval=15)
scp_opts=(-q -r -i "$dir/id_ed25519" -P "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
  -o LogLevel=ERROR)

fetch_base() {
  [[ -f $base ]] && return
  curl -fL -o base.part "$url"
  case $distro in
    arch) curl -fsSL "$url.SHA256" | sed 's/ .*/  base.part/' | sha256sum -c ;;
    debian) curl -fsSL "${url%/*}/SHA512SUMS" | awk -v f="${url##*/}" '$2 == f {print $1 "  base.part"}' | sha512sum -c ;;
    fedora) curl -fsSL "${url%/*}/Fedora-Cloud-44-1.7-x86_64-CHECKSUM" \
      | awk -v f="${url##*/}" '$2 == "(" f ")" {print $4 "  base.part"}' | sha256sum -c ;;
  esac
  mv base.part "$base"
}

create() {
  [[ $distro == nixos ]] && { echo "build the NixOS image first (nixos-image.sh)" >&2; exit 1; }
  mkdir -p "$dir"; cd "$dir"
  fetch_base
  [[ -f id_ed25519 ]] || ssh-keygen -q -t ed25519 -N '' -f id_ed25519
  # The guest password only matters on the VM's own console, which nobody sees.
  cat > user-data <<EOF
#cloud-config
users:
  - name: atlas
    groups: [wheel, sudo]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: false
    plain_text_passwd: atlas
    ssh_authorized_keys:
      - $(cat id_ed25519.pub)
EOF
  printf 'instance-id: atlas-%s-1\nlocal-hostname: atlas-%s\n' "$distro" "$distro" > meta-data
  cloud-localds seed.iso user-data meta-data
  qemu-img create -q -f qcow2 -b "$base" -F qcow2 disk.qcow2 "$size"
}

running() { [[ -f $dir/qemu.pid ]] && kill -0 "$(cat "$dir/qemu.pid")" 2>/dev/null; }

case ${1:-} in
  up)
    [[ -f $dir/disk.qcow2 ]] || create
    running && { echo "VM already running (pid $(cat "$dir/qemu.pid"))"; exit 0; }
    seed=(); [[ -f $dir/seed.iso ]] && seed=(-drive "file=$dir/seed.iso,if=virtio,media=cdrom")
    qemu-system-x86_64 -enable-kvm -cpu host -smp "$cpus" -m "$mem" \
      -drive "file=$dir/disk.qcow2,if=virtio" "${seed[@]}" \
      -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$port-:22" \
      -device virtio-rng-pci -device virtio-vga,xres=1366,yres=768 \
      -device qemu-xhci -device usb-tablet -device usb-kbd \
      -display none -qmp "unix:$dir/qmp.sock,server=on,wait=off" \
      -serial "file:$dir/serial.log" -daemonize -pidfile "$dir/qemu.pid"
    for _ in $(seq 180); do
      ssh "${ssh_opts[@]}" atlas@127.0.0.1 true 2>/dev/null && { echo "VM $distro up on port $port"; exit 0; }
      sleep 2
    done
    echo "VM did not answer on ssh; see $dir/serial.log" >&2; exit 1 ;;
  ssh) shift; exec ssh "${ssh_opts[@]}" atlas@127.0.0.1 "$@" ;;
  put) exec scp "${scp_opts[@]}" "$2" "atlas@127.0.0.1:$3" ;;
  get) exec scp "${scp_opts[@]}" "atlas@127.0.0.1:$2" "$3" ;;
  qmp) shift; exec python3 "$here/qmp.py" "$dir/qmp.sock" "$@" ;;
  down)
    running || exit 0
    ssh "${ssh_opts[@]}" atlas@127.0.0.1 sudo systemctl poweroff 2>/dev/null || true
    for _ in $(seq 60); do running || { rm -f "$dir/qemu.pid"; exit 0; }; sleep 1; done
    kill "$(cat "$dir/qemu.pid")"; rm -f "$dir/qemu.pid" ;;
  destroy) "$0" "$distro" down; rm -rf "$dir" ;;
  *) sed -n '2,13p' "$0"; exit 2 ;;
esac
