#!/bin/sh
# Boot Rainbow Robotics' Virtual Control Box under QEMU, for `make check-vm` and by hand: `sh sim/run-vm.sh <the converted disk> [directory]`.
#
# The box is the maker's own control-box software in a virtual machine (RBVirtualSimulator 8.4.4, an OVA of 2.1 GB that the maker publishes to its customers; the address is a share that does not stay still, so nothing here
# fetches it). Unpack the OVA (it is a tar: an .ovf, a .vmdk and a .mf with the disk's SHA-1, which is worth checking), then convert the disk once:
#
#     qemu-img convert -f vmdk -O qcow2 RBVirtualSimulator8.4.4-disk001.vmdk rb844.qcow2
#
# What this does is the arrangement that was found to work: the disk is attached as an IDE drive and run in snapshot mode (the converted image is never written), on an e1000 network card (the appliance's network setup names
# enp0s3, which is the name that card's device presents as), with its command and data ports (5000 and 5001) forwarded to 127.0.0.1:15000 and :15001 and to nothing else. KVM is used where there is any. The serial console
# goes to console.log in the directory. The box takes a minute or two to come up; `make check-vm` says whether it is.
#
# This starts a virtual machine that runs software from the maker, with a network of its own and nothing of this machine's shared; it is the maker's simulation of an arm, and it moves no hardware.
set -eu

DISK=${1:?the converted disk (rb844.qcow2)}
DISK=$(cd "$(dirname "$DISK")" && pwd)/$(basename "$DISK")
DIR=${2:-$(dirname "$DISK")}
ACCEL=
if [ -w /dev/kvm ]; then ACCEL="-enable-kvm -cpu host"; fi
cd "$DIR"
: > console.log
# shellcheck disable=SC2086
exec qemu-system-x86_64 $ACCEL -m 4096 -smp 4 \
  -drive "file=$DISK,format=qcow2,if=ide" -snapshot \
  -netdev "user,id=n0,net=10.0.2.0/24,dhcpstart=10.0.2.15,hostfwd=tcp:127.0.0.1:15000-10.0.2.7:5000,hostfwd=tcp:127.0.0.1:15001-10.0.2.7:5001" \
  -global PIIX4_PM.acpi-root-pci-hotplug=off -device e1000,netdev=n0,addr=0x3 \
  -display none -serial file:console.log
