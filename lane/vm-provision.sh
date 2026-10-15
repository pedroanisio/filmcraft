#!/usr/bin/env bash
# Create and provision an ephemeral lab build VM for this lane, then point vm.env at it.
# usage: vm-provision.sh <vmid> <name> [node] [cores] [mem_mb] [disk]
#   e.g. vm-provision.sh 121 fc-build-02 pve-note-1 14 24576 200G
# Nothing lives only on the VM: checkouts are re-shipped by vm-sync.sh, sr-core is re-shipped from the local repo.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/vm.env"
ID=${1:?vmid} NAME=${2:?name} NODE=${3:-pve-note-1} CORES=${4:-14} MEM=${5:-24576} DISK=${6:-200G}
case $NAME in *[!a-z0-9-]*|'') echo "bad VM name: $NAME" >&2; exit 2 ;; esac
case $ID in *[!0-9]*|'') echo "bad vmid: $ID" >&2; exit 2 ;; esac
SR_CORE=/home/admin/src/sr-core SR_REV=babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc
ctl() { ssh -F /dev/null -o BatchMode=yes "$FC_CTL" "cd ~/pve-lab && $*"; }
vm() { ctl "./lab.sh ssh $NAME $(printf '%q' "bash -lc $(printf '%q' "$1")")"; }

ctl "./lab.sh clone $ID $NAME $CORES $MEM $NODE"
ctl "./lab.sh api PUT /nodes/$NODE/qemu/$ID/resize disk=scsi0 size=$DISK >/dev/null && ./lab.sh api PUT /nodes/$NODE/qemu/$ID/config tags='debian;dev;filmcraft;ephemeral' >/dev/null"
vm 'sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq cloud-guest-utils pkg-config libasound2-dev libgtk-3-dev libx11-dev libxkbcommon-dev libwayland-dev libdbus-1-dev libxcursor-dev libxrandr-dev libxi-dev libssl-dev python3-lxml mesa-vulkan-drivers libvulkan1 >/dev/null; sudo growpart /dev/sda 1 >/dev/null 2>&1 || true; sudo resize2fs /dev/sda1 >/dev/null 2>&1 || true; df -h / | tail -1'
vm 'rustup update stable >/dev/null 2>&1; rustup component add clippy rustfmt && rustup target add wasm32-unknown-unknown && cargo --version'
vm '[ -d ~/craft-fonts ] || git clone -q https://github.com/storytold/craft-fonts ~/craft-fonts; ls ~/craft-fonts | head -3'

# sr-core (schema oracle + vpkg verifier) at the pinned revision, shipped from the local repo
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
git -C "$SR_CORE" archive -o "$TMP/sr-core.tgz" "$SR_REV"
scp -F /dev/null -q -o BatchMode=yes "$TMP/sr-core.tgz" "$FC_CTL:sr-core.tgz"
ctl "./lab.sh scp ~/sr-core.tgz $NAME:sr-core.tgz >/dev/null"
vm "rm -rf ~/sr-core && mkdir -p ~/sr-core && tar xzf ~/sr-core.tgz -C ~/sr-core && echo $SR_REV > ~/sr-core/.rev && python3 -c 'import lxml; print(\"lxml ok\")'"

sed -i "s/^FC_VM=.*/FC_VM=$NAME/" "$HERE/vm.env"
FC_SYNC_HEAD_ONLY=1 "$HERE/vm-sync.sh"
ctl "./lab.sh mark-derived $NAME '~/filmcraft' '~/sr-core' '~/craft-fonts' >/dev/null"
echo "provisioned $NAME ($ID on $NODE); vm.env now points at it"
